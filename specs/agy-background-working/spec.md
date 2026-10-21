# Spec: agy-background-working

## Meta
- **Created**: 2026-10-19
- **Type**: dev
- **Status**: implemented (T1–T3, TT1–TT4); R0.1 manual check pending
- **Approved by**: user ("Could you implement?", after the unattended stop)
- **Approved at**: 2026-10-20
- **Mode**: unattended
- **Branch**: fix/agy-background-working
- **Amends**: `specs/agent-card-status/spec.md` D13 (and its "Accepted cost of D13" Known Gap)

## Goal
An Antigravity (`agy`) sidebar card reads "Working", not "Ready", while the agent is waiting
on its own background work (a background `run_command`, a subagent, a `manage_task`) after a
tool-less reply.

Observed 2026-10-19 00:41 UTC on transcript `brain/2091e717-…/transcript.jsonl`: the last steps
were `PLANNER_RESPONSE[run_command]` → `GENERIC status:RUNNING` ("Tool is running as a
background task") → `PLANNER_RESPONSE[manage_task]` → `GENERIC DONE` → `PLANNER_RESPONSE`
tool-less "Awaiting completion notification." Nothing was appended while `cargo test
--workspace` ran, and the card read Ready from 10 s after the last write.

## Non-goals
- The Claude, Codex, Gemini and Aider paths. Only the Antigravity transcript classifier changes.
- `state_from`, `AgentState`, `LastMessage`, the card's precedence (`resolve_status`) and the
  waiting band. The fix produces an existing classification (`AssistantBusy`) and nothing
  downstream changes.
- The *other* half of D13's accepted cost: a genuine turn end that followed background work
  still reads `Ready` rather than `Waiting`. This spec touches only replies with background
  work still **pending**.
- A time limit on how long pending work may hold `Working` (rejected in D6).
- Reading task logs (`.system_generated/tasks/*.log`) or any file besides the transcript.

## Confirmed Goal
When the last meaningful step of an Antigravity transcript is a tool-less reply, and a
background task or subagent started since the last user input has **not yet reported back**,
the card reads **Working** for as long as that stays true, instead of falling to Ready 10 s
after the last write. When everything the turn started has reported back, behaviour is
exactly today's. Done when:
- a transcript shaped like the observed one classifies as `AssistantBusy` (→ `Working`), at
  any age;
- appending the task's completion notice returns it to today's classification (`None`);
- no transcript that reads anything other than `Waiting` today reads `Waiting` after the change;
- the corpus measurement in Research still holds for the implemented rule (inferred);
- `docs/architecture.md` G8 names the new transcript strings this depends on (inferred:
  an external format is recorded there whenever the card reads one).

## Research
- **The cause.** `finished_turn` returns `None` whenever `turn_was_foreground` is false
  (`crates/ai-monitor/src/antigravity/tail.rs:206-209`). `turn_was_foreground` walks back from
  the end of the back-scan window and returns `false` at the **first** non-foreground step
  (`tail.rs:217-234`). `None` reaches `state_from`'s recency fallback: `Busy` within
  `BUSY_WINDOW` (10 s) of the transcript's `mtime`, then `Idle` → card `Ready`
  (`crates/app/src/tabmeta/activity.rs:66-90`).
- **D13 already rejected the naive fix.** `specs/agent-card-status/spec.md:410-436`: "Rejected:
  `Working` until the next step — those 34 turn ends would read `Working` until the user typed
  again." So "any background work → Busy" (the request's first proposal) re-opens a rejected
  option. The accepted-cost Known Gap (`spec.md:500-503`) names this exact bug: "a reply written
  *while* background work still runs reads `Ready` from 10 s after it until the next step".
- **Antigravity pairs every background start with a completion notice** (read from the corpus
  2026-10-19 07:05–07:10 UTC):
  - a background task's start is a step with `status: "RUNNING"` whose `content` contains
    `Tool is running as a background task with task id: <conv>/task-<n>`;
  - its end is a `SYSTEM_MESSAGE` step whose content carries the header line
    `[Message] timestamp=<ts> sender=<conv>/task-<n> priority=…` and the text
    `Task id "<conv>/task-<n>" finished with result:`;
  - a subagent's start is the result step of an `invoke_subagent` dispatch (type `GENERIC` in
    current `agy`, `INVOKE_SUBAGENT` in the repo's fixture) whose content begins
    `Created At: …\nCompleted At: …\nCreated the following subagents:` followed by JSON objects
    carrying `"conversationId": "<uuid>"`;
  - a subagent's report is a `SYSTEM_MESSAGE` with header `sender=<uuid>`.
- **Measurement of the pending rule** (scratch script, 120 transcripts, 2026-10-19 07:10 UTC,
  216 tool-less replies with a `USER_INPUT` before them). "bg" = D13's verdict (not all
  foreground); "pending" = a start since the last `USER_INPUT` with no completion before the
  reply; outcome = next non-bookkeeping step:

  | | continued (more model steps) | ended (`USER_INPUT`) | end of file |
  |---|---|---|---|
  | bg, pending | **48** | 1 | 1 |
  | bg, nothing pending | **0** | 17 | 33 |
  | foreground | 0 | 38 | 53 |

  Every mid-work reply (48/48) is pending; 2 of 52 turn ends after background work are
  pending. Those two: `e7ef5a4a…:278`, the agent started `cargo run -p app` in the background
  for the user to try (the process really was still running), and `1ec84f5f…:81`, a subagent's
  own transcript ending with an abandoned background `grep` that never reported.
- **Anchoring is required.** A first, unanchored matcher (substring `Created the following
  subagents`) produced a false pending on `10b82d83…:96`: a `run_command` whose *output* quoted
  other transcripts. The anchored header match removed it (the table above is the anchored
  run).
- **Existing tests that touch this.** `crates/ai-monitor/tests/antigravity.rs:966-1060`
  (R2.1–R2.3) use synthetic `RUNNING` steps with no task-id text and dispatches with plain
  `GENERIC` results: none carry a start marker, so all keep `None`.
  `r2_2_the_real_fixtures_reply_after_a_subagent_is_no_robust_signal` (`antigravity.rs:1015`)
  reads `tests/fixtures/antigravity_transcript.jsonl` through step 12, whose step 11 is an
  `INVOKE_SUBAGENT` with the anchored subagent header and no report before the reply. It
  becomes `AssistantBusy` under this change and must be amended.
- **Cost budget.** The back-scan (`PROMPT_SCAN_CAP`, 1 MiB) runs only after a tool-less reply on
  a changed transcript (D14, R2.7 test `tail.rs:347-388`). The change re-uses that window;
  it walks further within it but reads no extra bytes.
- **Downstream mapping.** `LastMessage::AssistantBusy` → `AgentState::Busy` at any age
  (`activity.rs:77-79`) → `CardStatus::Working` (`crates/app/src/sidebar_model.rs` `resolve_status`).
  `Busy` never enters the waiting band.
- **External-format register.** `docs/architecture.md` G8 lists the Antigravity transcript
  vocabulary the card reads ("`status` (`RUNNING` = a background task)").
- **Commands.** `cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D
  warnings`; `cargo mutants -f crates/ai-monitor/src/antigravity/tail.rs -j 4` (CLAUDE.md:
  record survivors, not a ratio).

## L2 Checkpoints
Complexity: Medium (2 signals): an external format read as data (Antigravity transcript
strings); a small start/finish pairing state. Brownfield, so Implementation auto-resolves from L1.

| Dimension | Checkpoints | Resolved by |
|---|---|---|
| Core Behavior | pending → which state (D1); what "started" is (D2); what "finished" is (D3); interaction with D13's foreground verdict (D1) | request + L1 |
| Scope | which agents (Non-goals); which window (D4); downstream changes (D5) | request + L1 |
| Error/Edge | quoted strings in output (D2); unreadable line / window edge (D4); work that never reports (D6); format rename (D7); start/completion id shape mismatch (D2); an id started twice → set semantics, measured 0 (D3); a task message that is not its end → none measured, 57/57 terminal (D3) | L1 + assumed |
| Data Model | ids compared as exact strings (D2/D3); no new `LastMessage` variant (D5) | L1 + assumed |
| Implementation | where tests live, fixture, mutation gate (D8) | L1 |

Composite 1.0 (all resolved; D4, D6 and D7 `assumed`). Inversion probe: "every requirement met,
still fails?": (a) a completion string renamed by `agy` leaves every background turn pending →
Working until the next step, which is D13's rejected outcome. For tasks this is addressed by D3's
two independent recognisers. Subagents have only one, so the risk is accepted there and named
in D7 and Known Gaps. (b) A start id parsed with a different shape from its completion id
never matches it, so the start is pending forever. Addressed by D2's pinned id grammar (added
after L2 review, as an Error/Edge checkpoint). (c) A subagent that sends a progress message and keeps
working reads as reported → today's behaviour, which is the safe direction (D3).

## Decisions

### D1: A tool-less reply with background work still pending is `AssistantBusy`. This amends agent-card-status D13
- **Status**: resolved (request, refined by L1)
- **Decision**: when the last meaningful step is a tool-less `PLANNER_RESPONSE` (or one with
  `tool_calls: []`), the back-scan also tracks background **starts** (D2) and **completions**
  (D3). If at least one start has no completion, the classification is
  `LastMessage::AssistantBusy`. Otherwise the result is exactly today's: `AssistantEndTurn`
  when every step since the last `USER_INPUT` is foreground work, else `None`. Pending
  outranks D13's foreground verdict: an unlisted tool or an unknown step kind in the same turn
  does not stop a pending start from reading Busy. D13's "no robust signal" outcome is
  narrowed, not removed: it still covers background work that has fully reported, unlisted
  tools and kinds with nothing pending, and a window with no `USER_INPUT` and nothing pending.
- **Rationale**: measured, 48 of 48 mid-work replies have a start with no completion. Only 2 of
  52 turn ends after background work do, and in one of those a process the agent started
  really was still running.
  - **Rejected: any background work → Busy** (the request's first proposal, and D13's rejected
    "`Working` until the next step"). It reads all 52 turn ends after background work as
    Working until the user types, the cost D13 refused. D1 reads 2 of those 52 as Working.
    "Turn end" counts both the `USER_INPUT` rows (18) and the end-of-file rows (34). Some
    end-of-file rows may be live sessions rather than finished turns, so 52 is an upper bound.
  - **Rejected: keep D13 as is.** The bug stands: the card reads Ready during every background
    wait longer than 10 s (up to 96 s measured, and minutes for a `cargo test`).
  - **Rejected: pending → `Waiting`.** The agent is not waiting on the human; that would be the
    worst failure this project names.
  - **Rejected: task-log liveness** (the mtime of `.system_generated/tasks/task-<n>.log`, read
    to decide whether a task is still running). It would bound D6's hold, but four reasons
    each rule it out:
    - **It breaks the byte budget.** That is a stat per pending task on every refresh, outside
      the one transcript `stat` the worker pays today (Constraints; R6.7's single-stat rule).
    - **Subagents have no task log**, so half of the background kinds would still need pairing.
    - **A quiet command writes nothing.** `cargo` linking, a `sleep`, a `schedule` timer, or a
      server waiting for a request goes silent for minutes, so "stale log" does not mean
      "ended". The cap would flip to Ready mid-task, which is this bug.
    - **It adds a second file layout** (`tasks/`) to the external-format register for a signal
      the transcript already gives exactly: the completion notice. Pairing reads the definitive
      end and costs no new I/O.

### D2: "Started" is recognised only from the start step's own header
- **Status**: resolved (L1)
- **Decision**: a start is either
  1. a step whose `status` is `"RUNNING"` and whose `content` (a string) *begins* with the
     header `Created At: <line>\n` followed directly by
     `Tool is running as a background task with task id: `, immediately followed by a **task id** of the
     exact shape `<conversation uuid>/task-<one or more ASCII digits>`. A uuid is 36 characters
     of lowercase hex and `-`. The id ends at the last digit, so anything after it (a newline,
     `.`, `,`, `"`) is not part of it. Text after the marker that does not have that shape
     starts nothing. Verbatim from the corpus:
     `Tool is running as a background task with task id: 2091e717-a2d0-4967-9d63-96b8a5a868b3/task-451\nTask Description: cargo test --workspace`.
     Measured 2026-10-19 07:18 UTC: the marker text occurs 60 times. 58 are real starts, all in
     `RUNNING` steps (41 `GENERIC`, 17 `RUN_COMMAND`), all at the content's start, and all with
     this id shape. The other 2 are quoted text inside `DONE` steps (one `GENERIC` command
     result, one `SYSTEM_MESSAGE`), and both guards below exclude them; or
  2. a step whose `content` *begins* with the header `Created At: <line>\n`, optionally
     `Completed At: <line>\n`, then `Created the following subagents:`. Every
     `"conversationId": "<uuid>"` after that header, with a uuid of the shape above, is a
     **subagent id**.
  Completion ids (D3) are parsed with the same grammar, and ids are compared as exact strings.
  A `RUNNING` step without the task-id text starts nothing (it keeps D13's `None`, so the
  existing R2.1 tests stand).
- **Rationale**: measured. Each form has its own guard against quoted text:
  - **Form 2 is anchored at the content's start.** An unanchored substring match produced a
    false pending from a `run_command` whose output quoted another transcript
    (`10b82d83…:96`). A command result always begins with its own header followed by
    `The command exited…`, so the anchor excludes quoted text.
  - **Form 1 has two guards: `status: "RUNNING"` and the same content-start anchor.** All 58
    `RUNNING` steps in the corpus begin `Created At: …\nTool is running as a background task
    with task id: ` (2026-10-19 07:16 UTC), so the anchor costs no real start. A `DONE` step
    that quotes the task-start text starts nothing (wrong status), and neither does a `RUNNING`
    step that quotes it anywhere but at its start. The `RUNNING` steps also include
    `schedule` timers (`Task Description: Timer: 5s, Prompt: …`). Those are tasks like any
    other, and their firing message is their end (D3).
  - **The pinned grammar** ensures a start id and its completion id are cut the same way. An
    open-ended "run of non-whitespace" would swallow trailing punctuation, and the start would
    never match its completion.
  - **Rejected: keying on the dispatch name** (`invoke_subagent`, `run_command`
    backgrounded). The dispatch does not say which id it will get, so nothing could pair it with
    its completion.
  - **Rejected: keying on step `type`** (`INVOKE_SUBAGENT`). Current `agy` writes the subagent
    result as `GENERIC`, while the fixture has `INVOKE_SUBAGENT`. The header is the same in both.

### D3: "Finished" is any `SYSTEM_MESSAGE` from that id, recognised two independent ways
- **Status**: resolved (L1); the "any message" reading is `assumed`
- **Decision**: a `SYSTEM_MESSAGE` step's `content` completes an id if it has a line beginning
  `[Message] timestamp=<non-space> sender=<id>`, **or** it contains `Task id "<id>" ` (the
  quoted id followed by a space, whatever status word follows: `finished`, `was canceled`).
  Either recogniser alone is enough. Any message from a task's or a subagent's id counts as its
  end. Ids follow D2's grammar. Starts and completions are sets: a start seen twice is one
  pending id, and one completion clears it.
- **Measured** (2026-10-19 07:16 UTC, 120 transcripts): every one of the 57 messages from a
  task sender was terminal. 45 were `Task id "…" finished with result:`, 10 were
  `Task id "…" was canceled with result:` (by the user or by `manage_task`), and 2 were a
  `schedule` timer firing (`content=Check if cargo test completed`, from the timer's own task
  id, with no `Task id` text). No task id was started twice in any transcript.
- **Rationale**: for **tasks**, two recognisers mean a rename must break both before a finished
  task reads pending forever (the D13-rejected outcome, D7). The text recogniser accepts any
  status word because a `finished`-only match would leave canceled tasks (10 of 57) and
  timers (2 of 57, no text at all) with only one. Timers still have just the `sender=` header. **Subagents have only one**: the
  `sender=` header. A subagent's report body is free text (`content=TT4 — R4 …`), with no
  "finished" phrase in any of the corpus's subagent messages (2026-10-19 07:14 UTC). A rename of
  that header alone therefore makes every subagent turn read pending until the next step. This
  is accepted and named in D7 and Known Gaps, not claimed away. Treating any message from a subagent as its
  report errs toward "finished", which falls back to today's behaviour, never to a false
  Working or a false Waiting.
  - **Rejected: requiring `finished` text for tasks only.** That is one recogniser, so one rename
    re-opens the rejected outcome.
  - **Rejected: `manage_task` status results** (`Status: RUNNING` / done) as completions. They
    are polls written by the agent, not notices, and the corpus pairs every finished task with
    a `SYSTEM_MESSAGE`.

### D4: The walk runs over D14's existing window, newest first, and stops at the last `USER_INPUT` or the first unreadable line
- **Status**: assumed
- **Decision**: the back-scan window (`PROMPT_SCAN_CAP`, read only after a tool-less reply, as
  today) is walked from its end toward its start. Walking newest first, every completion is seen
  before the start it answers. The walk collects completed ids, and a start counts as pending
  if its id is not yet in that set. The walk ends at:
  - the last `USER_INPUT`. Starts before it do not count, the same boundary as D13.
  - an unreadable line or the start of the window. A pending start already found still reads
    Busy: everything newer than that start was walked, and a completion is always newer than its
    start, so the unreadable (older) line cannot be its completion. With nothing pending, the
    result is today's `None`.
  Steps after the reply (bookkeeping such as a `SYSTEM_MESSAGE` notice) are inside the window
  and walked like any other, so a completion that arrives after the reply clears the pending
  state immediately.
- **Rationale**: no new read, and the cost stays D14's (R2.7 holds unchanged). The early return
  at the first non-foreground step has to go, because a start may sit behind one. The walk is
  still bounded by the same window.
  - **Rejected: treating an unreadable line as "undecidable" outright** (today's
    `turn_was_foreground` rule). The window's first line is usually cut mid-record, so that rule
    would drop every pending start whose `USER_INPUT` lies beyond 1 MiB, and the argument above
    shows it is unnecessary.
  - **Rejected: carrying pending ids across turns** (counting starts before the last
    `USER_INPUT`). That needs state beyond the window. The measurement found no mid-work reply
    that depended on it (48/48 within the turn).

### D5: Reuse `LastMessage::AssistantBusy`; nothing downstream changes
- **Status**: assumed
- **Decision**: no new `LastMessage` variant, no change to `state_from`, `AgentState`,
  `resolve_status` or the band. Pending is classified inside `tail.rs` only.
- **Rationale**: `AssistantBusy` already means "dispatched and running" and maps to `Busy` at
  any age, which is the required behaviour. It is also what the same transcript reads as an
  instant earlier, while the background tool's dispatch is the last step.
  - **Rejected: a new `AssistantAwaitingBackground` variant**. It would touch the `ai-monitor`
    ↔ `app` seam and every exhaustive match for no observable difference, unless a time cap
    (D6) were added.

### D6: No time limit on pending work, and the residual risk is named
- **Status**: assumed
- **Decision**: a start with no completion holds `AssistantBusy` until one of three things
  happens: a matching completion lands (D3); the last meaningful step stops being a tool-less
  reply (a dispatch, a tool result or a `USER_INPUT` is appended, and today's table classifies
  it); or a new `USER_INPUT` moves the walk's boundary past the start (D4). Appending another
  tool-less reply, or bookkeeping that is not a matching completion, does **not** end it.
  There is no age cap.
- **Rationale**: the waits this exists for run from seconds to many minutes (a workspace
  `cargo test`), and any cap short enough to matter flips the card to Ready mid-task, which is
  this bug. **Residual risk, accepted:** background work that never reports (a killed task, an
  agent-launched long-lived process such as `cargo run -p app`) holds Working until the next
  step. Measured 2 of 52 turn ends. The consequence, stated plainly: **a hand-off reply after
  launching a long-lived process ("I've started the app, try Cmd+=") reads Working
  indefinitely**. The card hides that the turn is over and that the user is expected to act,
  until the process exits (its notice then clears it) or the user types. This is the price of
  not flipping to Ready during real waits, and it errs toward Busy, never toward `Waiting`.
  - **Rejected: cap at `STALE_LIMIT` (100 s)**. That needs a new variant (D5) and cuts real waits.

### D7: The new transcript strings are recorded in `docs/architecture.md` G8 as external-format assumptions
- **Status**: resolved (L1: G8 is where Antigravity vocabulary is registered)
- **Decision**: G8's Antigravity bullet names the four strings (the task-start header
  `Created At: …\nTool is running as a background task with task id: `, the subagent header
  `Created the following subagents:`, the `[Message] timestamp=… sender=<id>` header, and the
  text `Task id "<id>" ` (quoted id plus a space, any status word; renaming `finished` breaks
  nothing, renaming `Task id "` breaks this recogniser)) and how
  each rename degrades. A renamed **start** string reads as nothing pending, which is today's
  behaviour. For tasks, renaming **both** completion forms reads as pending forever: Working
  until the next step after every background turn. For subagents, renaming the **`sender=`
  header alone** does the same, because it is their only completion recogniser (D3). The oracle is the transcript itself: grep the newest
  `SYSTEM_MESSAGE` for the sender header. `specs/agent-card-status/spec.md` D13 gets an
  "Amended by `specs/agy-background-working` D1" note so the two specs do not silently disagree.
- **Rationale**: this is the same kind of fact as G5–G9: another program's output read as data,
  with nobody to say when it changes.

### D8: Tests and the mutation gate
- **Status**: resolved (request + L1)
- **Decision**: unit tests in `tail.rs`'s `mod tests` and integration tests through
  `ai_monitor::antigravity::tail_state` in `crates/ai-monitor/tests/`. A fixture models the
  observed step shapes (`RUNNING` start with task id, `manage_task` dispatch and status result,
  tool-less reply, then the completion `SYSTEM_MESSAGE`), with content trimmed to what the
  recognisers read. The existing fixture test at `antigravity.rs:1015` is amended to
  `AssistantBusy`. `cargo mutants -f crates/ai-monitor/src/antigravity/tail.rs -j 4` must end
  with 0 survivors that are not provably equivalent, and the count is recorded in this spec.
- **Rationale**: CLAUDE.md judges coverage by mutation, and the R2.1–R2.7 integration tests are
  the existing pattern.

## Constraints
- **Never `Waiting` from this path.** Pending yields only `AssistantBusy`; no change may add a
  route to `AssistantEndTurn` or `AssistantAsksUser` (agent-card-status Constraint
  "Conservative bias").
- **Byte budget unchanged.** The back-scan still runs only after a tool-less reply (R2.7); no
  additional file read.
- **`ai-monitor` stays read-only** over the transcript; no FD/`libc` code (AG11).
- **Dependency direction** unchanged: no `app` or GPU type enters `ai-monitor`.
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` clean.

## Known Gaps
- L2 review: 3 rounds, each NEEDS_FIX. Every issue raised was fixed in the spec. The third
  round's 3 issues (D7's string named as `finished`-only, D2's 60-vs-58 count, D1's "50") were
  fixed after the retry budget ran out and were not re-reviewed at L2. The L4 Spec Review
  covers them.
- Measured on one machine and one `agy` version (corpus of 120 transcripts, 2026-10-19
  07:10 UTC). A different `agy` version may phrase the strings differently (D7).
- A subagent that sends a progress message and keeps working reads as reported (D3), so its
  later reply falls back to today's recency. Not observed in the corpus.
- A task message that is a progress update rather than the task's end would read as its end
  (D3's "any message"), so the reply after it falls back to today's recency. None measured
  (57/57 terminal); the safe direction.
- Subagent completion has a single recogniser (the `[Message] … sender=<uuid>` header, D3). If
  `agy` renames it, every turn that started a subagent reads Working until its next step (D7).
- D1's 2-of-52 rate for "Working at a real turn end" belongs to this corpus. A user who often
  has `agy` start a dev server or `cargo run` for them to try will see it more often (D6).

## Requirements

Boundary for every code-behaviour sub-requirement below: the public function
`ai_monitor::antigravity::tail_state(path)`, read as its `TailState::last`. In every GWT,
"a turn" means a transcript whose steps after the last `USER_INPUT` are listed. "Start(T)" is
a `RUNNING` step with content `Created At: …\nTool is running as a background task with task
id: T\nTask Description: …`, and "Sub(S)" is a `GENERIC` step whose content begins `Created At:
…\nCompleted At: …\nCreated the following subagents:` followed by `{"conversationId": "S", …}`.
"Notice(id, body)" is a `SYSTEM_MESSAGE` whose content holds the line `[Message]
timestamp=2026-10-19T00:00:00Z sender=<id> priority=MESSAGE_PRIORITY_HIGH content=<body>`.
**Every placeholder id is grammar-valid (D2)**: `T`, `T1`, `T2`, `T'` are
`<36-char lowercase-hex uuid>/task-<digits>` (e.g. `2091e717-a2d0-4967-9d63-96b8a5a868b3/task-451`),
and `S`, `S1`, `S2` are 36-char uuids (e.g. `ed14342c-0000-4000-8000-00000000beef`). A test
must never use a literal `T` or `S`, because the shape rule would then reject it and the
anchor and status guards would go untested.

### R0: An agy card reads Working while the agent waits on its own background work — D1, D7, D8

#### R0.1: The observed transcript reads Working (manual)
- **Given**: a running `agy` tab whose transcript ends like the observed one: a background
  `run_command` with no completion yet, then a tool-less "Awaiting completion" reply
- **When**: more than 10 s pass with nothing appended
- **Then**: the card's status reads `Working`. When the completion notice lands and the agent
  continues, the card follows the agent as today

#### R0.2: The design is registered where the next reader looks (manual)
- **Given**: the change is merged
- **When**: a reader opens `docs/architecture.md` G8 and `specs/agent-card-status/spec.md` D13
- **Then**: G8 names the four strings and the degradation of each rename (D7), and D13 carries
  an "Amended by `specs/agy-background-working` D1" note. The mutation survivor count of
  `tail.rs` is recorded in this spec's `## Verification`

### R1: Pending background work reads AssistantBusy, finished work reads as today — D1, D3, D5, D6

#### R1.1: A task with no notice is pending
- **Given**: a turn `[dispatch run_command, Start(T), dispatch manage_task, GENERIC DONE, reply]`
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

#### R1.2: A task's notice by either recogniser clears it
- **Given**: the R1.1 turn plus, after the reply, (a) `Notice(T, "Task id \"T\" finished with
  result: …")`; (b) a notice with `sender=T` whose body has no `Task id` text (a timer firing);
  (c) a `SYSTEM_MESSAGE` with no `[Message]` header whose content contains
  `Task id "T" was canceled with result:`
- **When**: `tail_state` reads each
- **Then**: each returns `None` (today's D13 result for background work that has reported)

#### R1.3: A subagent with no report is pending; its report clears it
- **Given**: a turn `[dispatch invoke_subagent, Sub(S), reply]`, then the same plus `Notice(S, "TT4 — …")`
- **When**: `tail_state` reads each
- **Then**: the first returns `Some(AssistantBusy)`, the second `None`

#### R1.4: Several starts: any one unreported keeps it pending
- **Given**: a turn starting tasks T1 and T2 and subagents S1 and S2. Case A has notices for
  T1, T2 and S1 only. Case B has notices for all four
- **When**: `tail_state` reads each
- **Then**: A returns `Some(AssistantBusy)`; B returns `None`

#### R1.5: A notice for a different id does not clear a start
- **Given**: the R1.1 turn plus `Notice(T', …)` where `T'` differs from `T` only in its task
  number (`…/task-452` vs `…/task-451`)
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

#### R1.6: Pending outranks an unlisted tool or kind
- **Given**: the R1.1 turn with a dispatch of `frobnicate` and a `SOME_NEW_KIND` step inserted
  before the reply
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

#### R1.7: Never Waiting, at any age
- **Given**: each of R1's transcripts whose expected result is `Some(AssistantBusy)` (R1.1,
  R1.3's first case, R1.4 case A, R1.5, R1.6), written to disk with its `mtime` set 1 hour in
  the past
- **When**: `tail_state` reads each
- **Then**: none returns `Some(AssistantEndTurn)` or `Some(AssistantAsksUser)`; each returns
  `Some(AssistantBusy)` (the classification does not depend on age)

#### R1.8: A start seen twice is one pending id
- **Given**: the R1.1 turn with `Start(T)` appearing twice, then one `Notice(T, …)` after the reply
- **When**: `tail_state` reads it
- **Then**: `last == None`

### R2: Only a start step's own header starts anything — D2

#### R2.1: Quoted start text starts nothing
- **Given**: a turn `[dispatch run_command, GENERIC DONE, reply]` where the `GENERIC DONE`
  content is `Created At: …\nCompleted At: …\n\nThe command exited with code 0.\nOutput:\n`
  followed by quoted `Tool is running as a background task with task id: T` and quoted
  `Created the following subagents:\n{"conversationId": "S"}`
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantEndTurn)`. Today's result stands: `run_command` and a `DONE`
  `GENERIC` are foreground work, and the quoted text starts nothing. **Control** (same test): with
  that step replaced by `Start(T)` (the same id, at the content's start of a `RUNNING` step) the
  result is `Some(AssistantBusy)`, and with it replaced by a `GENERIC` `DONE` step whose content
  *begins* with the subagent header naming `S` the result is `Some(AssistantBusy)`. So the test
  fails if the anchor accepts too little

#### R2.2: A RUNNING step that quotes the marker mid-content starts nothing
- **Given**: a turn `[dispatch run_command, RUNNING step whose content is "Created At: …\nsome
  output\nTool is running as a background task with task id: T", reply]`
- **When**: `tail_state` reads it
- **Then**: `last == None` (background work by D13, nothing pending). **Control** (same test):
  the same `RUNNING` step with the marker moved to directly after its `Created At:` line
  (i.e. `Start(T)`) gives `Some(AssistantBusy)`

#### R2.3: A RUNNING step without the marker starts nothing
- **Given**: a turn `[dispatch run_command, RUNNING GENERIC with content "Created At: …", reply]`
- **When**: `tail_state` reads it
- **Then**: `last == None`

#### R2.4: The id is cut at the last digit
- **Given**: three turns whose `Start` text is followed, right after the id, by `.`, by `,` and
  by end of content respectively, each with a `Notice(T, "Task id \"T\" finished…")`
- **When**: `tail_state` reads each
- **Then**: each returns `None` (the start matched its notice)

#### R2.5: Text that does not have the id shape starts nothing
- **Given**: a turn whose `RUNNING` step reads `…with task id: not-a-task-id\n…`, then a reply
- **When**: `tail_state` reads it
- **Then**: `last == None`

### R3: The walk's boundaries — D4

#### R3.1: Starts before the last USER_INPUT do not count
- **Given**: `[USER_INPUT, Start(T), reply, USER_INPUT, dispatch view_file, VIEW_FILE, reply]`
  with no notice for T
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantEndTurn)` (today's result for the second turn)

#### R3.2: A pending start found before the window's edge reads Busy
- **Given**: a transcript whose last `USER_INPUT` lies more than `PROMPT_SCAN_CAP` bytes before
  the end, with `Start(T)` inside the final `PROMPT_SCAN_CAP` bytes, then a reply
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`. Without the `Start(T)` it is `None` (today)

#### R3.3: A notice after the reply clears the start
- **Given**: the R1.1 turn plus `Notice(T, …)` and a `CHECKPOINT` after the reply
- **When**: `tail_state` reads it
- **Then**: `last == None`

#### R3.4: Another reply or non-matching bookkeeping does not end the hold
- **Given**: the R1.1 turn plus a second tool-less reply and a `CHECKPOINT`, with no notice
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

#### R3.5: The back-scan's byte budget is unchanged (manual)
- **Given**: a transcript larger than `TAIL_CAP` ending in a pending turn's reply, and the
  same transcript ending in each non-reply kind of R2.7 (agent-card-status)
- **When**: the classification runs through the counting byte source (`state_from_reads`, private
  to `tail.rs`, so this is checked by the unit tests in `tail.rs`'s `mod tests`: the existing
  `r2_7_*` pair plus a pending-reply case added by T1)
- **Then**: the reply case reads at most `TAIL_CAP + PROMPT_SCAN_CAP` bytes, and each
  non-reply case at most `TAIL_CAP`

### R4: Real step shapes — D8

#### R4.1: A fixture modelled on the observed transcript reads Busy, then clears
- **Given**: the new fixture `crates/ai-monitor/tests/fixtures/tt4_antigravity_background_pending.jsonl`,
  modelled on `2091e717…` steps (`USER_INPUT`, `PLANNER_RESPONSE[run_command]`, `GENERIC
  RUNNING` with the verbatim start text, `PLANNER_RESPONSE[manage_task]`, `GENERIC DONE`
  `Status: RUNNING`, tool-less "Awaiting completion notification." reply), then the same with
  the observed `SYSTEM_MESSAGE` completion appended
- **When**: `tail_state` reads each
- **Then**: the first returns `Some(AssistantBusy)`, the second `None`

#### R4.2: The existing fixture's reply after an unreported subagent reads Busy
- **Given**: `crates/ai-monitor/tests/fixtures/antigravity_transcript.jsonl` through step 12
  (step 11 is an `INVOKE_SUBAGENT` with the subagent header and no report)
- **When**: `tail_state` reads it
- **Then**: `last == Some(AssistantBusy)`

## Tasks

TT tasks create NEW files only (`crates/ai-monitor/tests/tt{n}_*.rs`, collected by
`cargo test --workspace`). Each reaches only the public boundary
`ai_monitor::antigravity::tail_state` plus `ai_monitor::activity::{LastMessage,
TAIL_CAP, PROMPT_SCAN_CAP}`, and writes its transcripts to a per-test directory under
`std::env::temp_dir()` the way `crates/ai-monitor/tests/antigravity.rs` does. They are
written against the GWTs before T1 lands. **Expected red before T1** (the new behaviour):
R1.1, R1.3's first case, R1.4 case A, R1.5, R1.6, R1.7, R2.1's and R2.2's **Control** cases
(each written as its own named test), R3.2's first case, R3.4, R4.1's first case, R4.2.
**Green before and after T1** (regression guards: today's `None` or `AssistantEndTurn` must
survive): R1.2, R1.3's second case, R1.4 case B, R1.8, R2.1's and R2.2's main cases, R2.3–R2.5, R3.1,
R3.2's second case, R3.3, R4.1's second case. A red-first check applies only to the first list.

### TT1: acceptance tests for R1's GWTs
- **Fulfills**: R1 (tests)
- **Depends on**: (none)
- **Files**: new `crates/ai-monitor/tests/tt1_agy_pending_background.rs`. R1.7 sets the
  `mtime` with `std::fs::File::set_modified`.

### TT2: acceptance tests for R2's GWTs
- **Fulfills**: R2 (tests)
- **Depends on**: (none)
- **Files**: new `crates/ai-monitor/tests/tt2_agy_start_anchoring.rs`

### TT3: acceptance tests for R3's GWTs
- **Fulfills**: R3 (tests)
- **Depends on**: (none)
- **Files**: new `crates/ai-monitor/tests/tt3_agy_walk_boundaries.rs` (R3.1–R3.4; R3.5 is
  manual and belongs to T1's unit tests)

### TT4: acceptance tests for R4's GWTs
- **Fulfills**: R4 (tests)
- **Depends on**: (none)
- **Files**: new `crates/ai-monitor/tests/tt4_agy_real_shapes.rs` and new fixture
  `crates/ai-monitor/tests/fixtures/tt4_antigravity_background_pending.jsonl`. The fixture is
  modelled on
  `~/.gemini/antigravity-cli/brain/2091e717-a2d0-4967-9d63-96b8a5a868b3/.system_generated/logs/transcript.jsonl`
  step indices 447–455. If that file has aged out, use the verbatim strings quoted in D2 and the
  completion content below. The completion's `content` opens with a preamble before its header:
  `The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the
  system as important information to pay attention to.\n\n<SYSTEM_MESSAGE>\n[Message]
  timestamp=2026-10-19T07:05:10Z sender=2091e717-a2d0-4967-9d63-96b8a5a868b3/task-451
  priority=MESSAGE_PRIORITY_HIGH content=Task id "2091e717-a2d0-4967-9d63-96b8a5a868b3/task-451"
  finished with result:\n…\n</SYSTEM_MESSAGE>`. The steps are (file lines 448–456: the `USER_INPUT`
  through the completion `SYSTEM_MESSAGE`). It keeps the verbatim start text, the
  `manage_task` dispatch, the `Status: RUNNING` poll result and the reply, plus a second
  variant with the completion `SYSTEM_MESSAGE` appended. Long output is cut to a few lines,
  and nothing from outside this repo's paths is kept.

### T1: pending-background classification in `tail.rs` [vertical]
- **Fulfills**: R0, R1, R2, R3, R4
- **Depends on**: TT1, TT2, TT3, TT4
- **Files**: `crates/ai-monitor/src/antigravity/tail.rs`; `crates/ai-monitor/tests/antigravity.rs`
  (amend only `r2_2_the_real_fixtures_reply_after_a_subagent_is_no_robust_signal` to expect
  `Some(LastMessage::AssistantBusy)` and rename it to say so, per R4.2)
- **Work**:
  - Replace `turn_was_foreground(&str) -> bool` with a walk returning a three-way verdict
    (`Foreground` → `AssistantEndTurn`, `Pending` → `AssistantBusy`, otherwise → `None`). It
    walks newest first, collects completion ids from `SYSTEM_MESSAGE` steps (D3), counts a start
    (D2) as pending when its id is not yet completed, and does not return early at a
    non-foreground step. It stops at the last `USER_INPUT`, an unreadable line, or the window's
    start (D4).
  - `finished_turn` maps the verdict. `classify` and `last_step` stay as they are.
  - Keep the closed-list style and doc comments of the surrounding code. Each recogniser
    (task-start header, subagent header, `sender=` header, `Task id "<id>" ` text) is a small
    named function with a doc comment citing this spec's D2/D3 and the measured counts. No
    regex crate: hand-written prefix/shape parsing (`ai-monitor` has no regex dependency).
  - Unit tests in `mod tests`: the R3.5 pending-reply byte-budget case, and any case needed
    to kill the mutants below.
  - The module doc comment (`tail.rs:1-24`) and `FOREGROUND_TOOLS`' comment cite D13 as amended
    by `specs/agy-background-working` D1.
- **Done when**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D
  warnings` pass; TT1–TT4 are green.

### T2: register the design in docs and amend D13 [vertical]
- **Fulfills**: R0
- **Depends on**: (none). Parallel with T1 (no shared files).
- **Files**: `docs/architecture.md` (G8's Antigravity bullet), `specs/agent-card-status/spec.md`
  (an "Amended by" note under D13 and on the "Accepted cost of D13" Known Gap line)
- **Work**: D7's four strings, each rename's degradation (including the single-recogniser
  subagent and timer cases), the oracle, and D6's residual risk in one sentence.

### T3: mutation gate and verification record [vertical]
- **Fulfills**: R0
- **Depends on**: T1, T2
- **Files**: this spec (a new `## Verification` section); new
  `crates/ai-monitor/tests/corpus_agy_pending.rs`; `crates/ai-monitor/src/antigravity/tail.rs`
  (new unit tests in `mod tests`, or a condition rewritten to remove an equivalent mutant)
- **Work**: run `cargo mutants -f crates/ai-monitor/src/antigravity/tail.rs -j 4`. Treat every
  survivor as a missing test (add it in `tail.rs`'s `mod tests`) unless it is provably
  equivalent. Prefer rewriting the condition over an `exclude_re` (CLAUDE.md). Record the
  final survivor count (not a ratio), the full test count, and clippy's result.
- **Corpus re-measurement** (Confirmed Goal, bullet 4): add a new `#[ignore]`d test
  `crates/ai-monitor/tests/corpus_agy_pending.rs`. It reads every
  `~/.gemini/antigravity-cli/brain/*/.system_generated/logs/transcript.jsonl` (skipping, not
  failing, when the directory is absent). For each tool-less reply after a `USER_INPUT`, it
  writes the transcript prefix ending at that reply to a temp file, calls `tail_state`, and
  tallies the result against the outcome (next non-bookkeeping step: model step / `USER_INPUT`
  / end of file). It asserts that no reply whose turn (since the last `USER_INPUT`) contains a
  `RUNNING` step or a subagent-header step classifies as `AssistantEndTurn` or
  `AssistantAsksUser` (the never-Waiting Constraint, on real data), and prints the table. Run it with
  `cargo test -p ai-monitor --test corpus_agy_pending -- --ignored --nocapture`, and record the
  table with its UTC time in `## Verification`. It holds if every "continued" reply after
  background work is Busy and the Busy turn ends stay a small minority, as in Research. If the
  numbers moved, record why rather than force them.

## External Dependencies

### Pre-work
(none)

### Post-work
- R0.1 is manual: launch the app (`/run-term`), start `agy` in a tab, have it run a long
  background command (e.g. `sleep 60` backgrounded), and confirm the card reads `Working`
  after a tool-less reply until the notice lands.
- Opening a PR is left to the person; unattended runs never push.

## Spec Review
- **Verdict**: NEEDS_FIX (final review, 3 of 3)
- **Reviews**: 3
- **Fixed**:
  - [Task order] R1.7 limited to R1's own Busy cases (it reached into TT3/TT4's transcripts).
  - [Readiness] T3's Files now list `tail.rs` and the new corpus test.
  - [Readiness] Tasks preamble splits the GWTs into expected-red and green-throughout.
  - [Testability] Placeholder ids pinned to D2's grammar; R2.1/R2.2 gained positive controls.
  - [Coverage] Confirmed Goal bullet 4 now has a task: the `#[ignore]`d corpus test in T3.
  - [Readiness] TT4 names the full transcript path and quotes the completion content.
  - [Task order] (after review 3) R2.1/R2.2 controls moved to the expected-red list. **This
    fix was not re-reviewed**: the review budget was spent.
- **Open**:
  (none known; the last fix is unreviewed)

## Verification
Recorded 2026-10-21 UTC, in the worktree `terminal-fix-agy-background-working`.

- **Red-first check.** TT1–TT4 were run against `main`'s `tail.rs` (4d60d74). Exactly the
  expected-red list failed (R1.1, R1.3, R1.4 A, R1.5, R1.6, R1.7, the R2.1 and R2.2 controls,
  R3.2, R3.4, R4.1, R4.2), and every regression guard passed.
- **Mutation gate.** `cargo mutants -f crates/ai-monitor/src/antigravity/tail.rs -j 4`: 90
  mutants, 84 caught, 6 unviable, **0 survivors**.
- **Tests.** `cargo test --workspace`: 2071 passed, 0 failed, 19 ignored.
- **Lint.** `cargo clippy --workspace --all-targets -- -D warnings`: clean.
  `cargo fmt --all -- --check`: clean.
- **Corpus re-measurement** (`cargo test -p ai-monitor --test corpus_agy_pending -- --ignored
  --nocapture`, 2026-10-21 02:30 UTC, 121 transcripts). "bg" means the turn had a `RUNNING`
  step or a subagent header since the last `USER_INPUT`:

  | turn | result | continued | ended | eof |
  |---|---|---|---|---|
  | bg | `AssistantBusy` | **49** | 1 | 1 |
  | bg | `None` | 0 | 15 | 13 |
  | fg | `AssistantEndTurn` | 0 | 40 | 52 |
  | fg | `None` | 0 | 3 | 21 |

  It holds. Every reply written mid-work after background work reads Busy (49/49), 2 turn
  ends do, and no background turn reads Waiting (the test asserts this). The `fg None` rows
  are turns with an unlisted tool such as `manage_task` and nothing pending, which is
  unchanged D13 behaviour.
- **Test helpers.** The TT files share `crates/ai-monitor/tests/agy_steps/mod.rs` (the
  Requirements' Start, Sub and Notice builders) rather than each repeating them.
- **R0.1** (manual, `/run-term` with a live `agy` tab) has not been run.
