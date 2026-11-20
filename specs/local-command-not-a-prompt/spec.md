# Spec: local-command-not-a-prompt

## Meta
- **Created**: 2026-11-14
- **Type**: dev
- **Status**: approved
- **Mode**: unattended
- **Branch**: fix/local-command-not-a-prompt
- **Approved by**: unattended (request marker)
- **Approved at**: 2026-11-15
- **Amended**: 2026-11-16 — D1 veto and R1.15, by the person after the Gate-3 escalation

## Goal
(Request, verbatim in substance.) The first paragraph of the request is a note to the
person about their tmux server: its global environment still carries a dead Claude Code
2.1.276 session's variables (`CLAUDECODE=1`, `CLAUDE_CODE_CHILD_SESSION=1`,
`TERM_PROGRAM=WarpTerminal`, a stale messaging socket), every new tmux session inherits
them, and `TERM_PROGRAM` therefore reads as Warp inside it. The remedy given there is the
person's own (`tmux set-environment -gu <VAR>` for each, or a tmux server restart).

"For the fix itself, the details to carry over are in the earlier message: what to change
in `activity.rs`, and the skill-command case to check before relying on the
`system/local_command` record."

The earlier message is not in this run's context. What it pointed at is reconstructed
from the branch name (`fix/local-command-not-a-prompt`), the code and the session logs:
a Claude Code **local** slash command (`/model`, `/effort`, `/clear`, `/context`, …) is
read by `crates/ai-monitor/src/activity.rs` as a human prompt, so the sidebar card reads
`Working` after one and never comes back, although Claude was not sent anything and will
not answer.

## Non-goals
- Clearing the tmux server's leaked environment. That is the person's action, named in
  the request; no code in this repo changes it. (It is a sibling of the
  `launching-the-app-from-Claude-Code-leaks-env` problem, not this one.)
- Skill commands (`/harness-ops:specify`, `/run-term`, …). They are prompts — Claude
  answers every one — and must stay prompts.
- `!` bash-mode commands (`<bash-input>` / `<bash-stdout>`). Measured: Claude answers them
  (4 of 4), so they stay prompts.
- `/context`'s trailing `isMeta` "## Context Usage" record and the manual `/compact`
  continuation summary (Known Gaps).
- The app-side state mapping (`crates/app/src/tabmeta/activity.rs::state_from`), the
  overview's buckets, and Antigravity. None changes; they consume `LastMessage` as before.
- The ai-monitor panel's skill list (`telemetry.rs::scan_command_name`), which counts every
  `<command-name>` as an invocation. A separate question.

## Confirmed Goal
A Claude Code local slash command does not change what a session's activity reads as. After
one, the sidebar card keeps the state it had before the command (typically `Waiting for
input` after an `end_turn`), the background-work hold (G8) is not forgotten because of it,
and the Agent Monitor's digest neither opens a turn nor moves "elapsed since the latest
human prompt" for it. Skill commands and `!` commands still count as prompts.

Done when:
- `ai_monitor::activity::tail_state` on a log that ends `end_turn` → a local command whose
  wrapper is a `user` record (the user form, e.g. `/effort`, and the mixed form, e.g.
  `/clear`) returns `last == Some(AssistantEndTurn)`; today it returns `UserPrompt`. (The
  pure system form already reads right today, because neither of its records has a
  `message.role`; it is a regression guard, not a red case.)
- The same log ending in a **skill** command instead still returns `UserPrompt`.
- A digest folded over `end_turn` → `/model` keeps its previous `last_prompt_ms` and turn
  count, and a turn still open before the command (an interrupt) is still open after it
  (`end_ms == None`).
- A log whose turn started background work and ended (`end_turn`) with the work still out,
  then `/model` (user form), returns `last == Some(AssistantBusy)` — the hold survives the
  command; today it returns `UserPrompt`.
- A handed-back `task-notification` (D27) followed by a local command still classifies the
  record before the notification.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass.

Inferences beyond the literal request, each marked:
- (inferred) The defect is the card/overview misreading local commands. The request only
  says "the fix" and names `activity.rs`; the branch name and the code fix the reading.
- (inferred) "The skill-command case to check before relying on the `system/local_command`
  record" means: show that a skill command never carries a local-command output record
  before using that record as the discriminator. Checked in Research — it never does, and
  the `system/local_command` record alone covers only 20 of 44 local commands, so the
  discriminator is the output record in **either** form (D1).
- (inferred) The overview digest (`telemetry.rs::digest_turn`) is in scope, because it
  shares `activity::is_human_prompt` by design (`session-overview-ui` D25).

## Research

Corpus readings are from `~/.claude/projects/**/*.jsonl` at **2026-11-14T23:51Z**, versions
2.1.277–2.1.325; the corpus moves by the hour, so the counts are a dated reading, not
reproducible constants.

### Code
- The tail classification is `last_meaningful` (`crates/ai-monitor/src/activity.rs:259`):
  newest first, the first record with `message.role` `assistant`/`user` decides; a user
  record goes to `classify_user` (`activity.rs:451`), which returns `UserPrompt` for
  anything without `toolUseResult` — it checks neither `isMeta` nor the text.
- The app maps `UserPrompt` → `AgentState::Busy` with no recency decay
  (`crates/app/src/tabmeta/activity.rs:66-91`, `state_from`): a precise `last` wins over
  the 10 s / 100 s recency rule, which only runs on `None`. So a misread prompt holds
  `Working` until the next real turn.
- `state_from_reads` (`activity.rs:104`) wraps `last_meaningful` with two wider-read rules
  that share one `PROMPT_SCAN_CAP` read: D27's `skip_handed_back` (`activity.rs:402`,
  gated on `newest_is_notification`, `activity.rs:439`) and the background hold
  `background_pending` (`activity.rs:477`, gated on `AssistantEndTurn`).
- `background_pending` stops at the first `is_human_prompt` record (`activity.rs:581`).
  `is_human_prompt` accepts a `<command-name>` wrapper and a `<local-command-stdout>` user
  record (no `toolUseResult`, not `isMeta`, no `origin`); its doc comment says so on
  purpose ("slash commands, `!` commands … all count", `activity.rs:578`).
- The digest's turn machine `digest_turn` (`crates/ai-monitor/src/telemetry.rs:1974`,
  called per record from `apply_claude`, `telemetry.rs:747`) opens a turn and sets
  `last_prompt_ms` at every `is_human_prompt` record, and closes an open turn there. It
  mutates `turns` (`VecDeque<Turn>`, capped at `TURNS_CAP` = 64), `turn_bg`, `turn_ended`
  and `last_prompt_ms` (`telemetry.rs:620-632`). It is a forward, streaming fold with no
  look-ahead.
- The tab label already skips command wrappers by text: `is_command_wrapper`
  (`activity.rs:228`) matches `<command-` and `<local-command-` prefixes and
  `user_prompt_text` (`activity.rs:210`) applies it. The test
  `prompts_skip_claude_code_command_wrappers` (`activity.rs:900`) carries the user-form
  `/effort` shape. Nothing else uses it.
- Public test boundary: `ai_monitor::activity::{tail_state, LastMessage}` and
  `ai_monitor::telemetry::{SessionTelemetry, Turn}` (`with_digest()`, `apply`,
  `snapshot()` → `last_prompt_ms`, `turns`), as `crates/ai-monitor/tests/tt8_digest_timeline.rs`
  and `tt5_claude_real_shapes.rs` use them. Existing integration tests already use the
  prefixes `tt1_`…`tt11_`, so new files need distinct names.
- `docs/architecture.md` G8 defines "the human prompt that bounds the turn"
  (`docs/architecture.md:300`) and records how each external string degrades.

### What a local command writes (corpus)
Shapes, oldest → newest (`U` user record, `Um` isMeta user, `S` system/`local_command`):

| form | records | seen as |
|------|---------|---------|
| user-form output | `Um:<local-command-caveat>` → `U:<command-name>/x…` → `U:<local-command-stdout>…` | `/effort`, `/model`, `/exit`, `/compact` (2.1.277–2.1.319) |
| mixed | `Um:<local-command-caveat>` → `U:<command-name>/clear…` → `S:<local-command-stdout></local-command-stdout>` (carries `commandRun`) | `/clear` (2.1.302–2.1.317) |
| system-form | `S:<command-name>/x…` → `S:<local-command-stdout>…` | `/model`, `/context`, `/login` |

- Every local command (44 of 44) has its output record as the **adjacent** record: 24 a
  `user` `<local-command-stdout>`, 20 a `system` `subtype: "local_command"`. So the
  `system/local_command` record alone misses 24 of 44, including every `/effort`.
- The output lands 0.04–15.1 s after the wrapper (two at 14–15 s; pickers). Until then the
  wrapper is alone on disk.
- 43 of 44 local commands are followed by a human prompt (26), another command (14) or
  EOF (3), never by an assistant answer to them; the one exception is `/login` typed while
  a turn was running.
- No `<local-command-stderr>` record exists in the corpus (0).
- `/context` writes one more record after its output: `Um:## Context Usage…` (2 seen,
  neither answered).
- A manual `/compact` writes the continuation summary (`isCompactSummary: true`) **before**
  the caveat and wrapper.
- Every `user`-record wrapper of a local command (36 of 36) has a `<local-command-caveat>`
  record (`isMeta: true`) as its **adjacent older** record, and every caveat (36 of 36) is
  followed directly by such a wrapper: one caveat per command, including the 14
  command-after-command runs. The pure system form (8) writes no caveat; its wrapper is a
  `system` record, which has no `message.role` and is not `type: "user"`, so today's code
  already ignores it. (Reading at 2026-11-15T00:01Z.)
- Adjacency was measured **line by line** for both markers: the caveat is the very next
  JSONL line before the wrapper (36 of 36) and the output the very next line after it (44 of
  44). No record of any type, `system` subtypes included, sat between them.
- Sizes (2026-11-15T00:04Z, 46 outputs): the largest output line is 3,072 bytes (a system-form
  `/context`), the largest caveat + wrapper + output group 4,183 bytes; 0 over `TAIL_CAP`
  (8 KiB). The 14 command-after-command runs stack groups, so a run of two or three
  commands can push the record before them out of the 8 KiB tail.

### What a skill command writes (corpus) — the check the request asked for
- 42 of 42 skill invocations: `U:<command-message>x</command-message><command-name>/x…`
  → `Um:Base directory for this skill: …` → an assistant record. **0 of 42** are followed
  by any local-command output record, in either form, and **0 of 42** are preceded by a
  `<local-command-caveat>` record.
- 39 of 42 carry `origin: {"kind": "human"}` and `turnOrigin`; 3 (2.1.31x) carry no
  `origin`. No local-command wrapper carries `origin` or `turnOrigin` — but neither do
  human prompts before 2.1.300 (518 plain user records with no `turnOrigin`), so absence
  is not a usable signal.
- The tag order differs (local wrappers open with `<command-name>`, skill wrappers with
  `<command-message>`), 44/44 and 42/42 — an incidental serialisation order, not a field.

### `!` commands (corpus)
- `U:<bash-input>` → `U:<bash-stdout>` → an assistant record, 4 of 4. They are answered.

## L2 Checkpoints

Complexity: **Medium** (2 signals) — external resource (another program's log format, read
as data: G8's kind of fact); state machine (the digest's turn machine and the newest-first
walks). Brownfield: Implementation auto-resolves from the existing module (weights Core 30,
Scope 20, Error 25, Data 15, Implementation 10). No checkpoint is fenced (no schema, data
migration, auth, payment or security surface).

| Dimension | Checkpoints | Resolved by |
|-----------|-------------|-------------|
| Core Behavior | what identifies a local command (D1); what the card reads after one (D2); the background hold across one (D4, D5); the digest across one (D6) | request + research; D2/D6 assumed |
| Scope | skill commands stay prompts (D5); `!` commands stay prompts (D5); app mapping unchanged (Non-goals); `/context` / `/compact` tails (Known Gaps) | research |
| Error/Edge | neither marker present (D3); records interleaved between wrapper and output (D7); window cut / half-written line, skipped records filling the tail (D9); stderr form (D8); two commands in a row (D4) | research; D7/D8 assumed |
| Data Model | the record shapes (Research table); which digest fields a wrapper mutates (D6) | research |
| Implementation | one shared filter, no new read, no new dependency (D4, D9); docs and tests (D10, D11) | L1 auto-resolve |

Unknown/unknown pass. Tier 1 actors: the person (types the command), Claude Code (writes
the records), the card (tail), the overview (digest) — each covered. Tier 2: D2 implies the
background walk must skip the same records (→ D4/D5); D1 implies a rule for a wrapper
with neither marker (→ D3) and for records between a marker and the wrapper (→ D7). Tier 3
(state machine): D6 × "a prompt closes an open turn" → a local command typed while a turn
is open must leave it open (→ D6, R3.3). Composite 1.00, every dimension ≥ 0.60, 0 unknowns pending.

Inversion probe: *every requirement met, still fails?* — a skill command misread as local
gives a false `Waiting` while Claude works, the worst failure this module can produce. D1
rules that out structurally: both of its markers are records a skill invocation never has
(0 of 42 for each), and the discriminator requires one of them.

## Decisions

### D1: A local command is recognised by a caveat before it or an output after it
- **Status**: resolved (request + research); revised after L2 review
- **Rationale**: Only a `user` record whose message text begins `<command-` (a **command
  wrapper**) can be misread, because only `type: "user"` records with a `message.role` reach
  the classification and `is_human_prompt`; the pure system form's wrapper is a `system`
  record and is already ignored, which is why D1 is scoped to `user` wrappers on purpose.
  Such a wrapper is a **local command** iff either:
  - its adjacent older record (D7) is a **caveat**: a `user` record whose message text
    begins `<local-command-caveat>`; or
  - its adjacent newer record (D7) is a **local-command output**: a `user` record whose
    message text begins `<local-command-stdout>` or `<local-command-stderr>`, or a `system`
    record with `subtype: "local_command"` whose `content` begins with one of those.

  The request asked for the skill-command case to be checked before relying on
  `system/local_command`. Checked: no skill invocation (0 of 42) has either marker; every
  user-form local wrapper (36 of 36) has the caveat and every local command (44 of 44) its
  output. `system/local_command` **alone** is rejected — it is the output form for only 20
  of 44, and every `/effort` writes a user-form `<local-command-stdout>`. The caveat is the
  marker each walk reads first in a forward fold and is present the moment the wrapper is
  (so the 0–15 s before an output lands, unbounded while a `/model` picker stays open, no
  longer read `Working`); the output is the one a newest-first walk meets first. Accepting
  either means a rename of one tag degrades only to the other, and a rename of both to
  today's reading. Also rejected: (a) the tag order (`<command-name>` first = local,
  `<command-message>` first = skill) — it holds today (44/44, 42/42) but is serialisation
  order, not a field; (b) `origin.kind` / `turnOrigin` absence — 3 skill invocations and
  every pre-2.1.300 human prompt also lack them, so it would read a skill command as local;
  (c) an allowlist of command names — open-ended (plugins add commands) and wrong in the
  dangerous direction for any skill that shares a name. **Steelman against D1** (raised by
  the L2 review): the output pairing alone leaves a window in which the wrapper reads as a
  prompt and forces the forward digest into save-and-undo machinery, while a marker written
  *before* the wrapper settles it at once. Answered by adopting that marker: the caveat is
  the primary signal in both directions, and pairing remains only as the fallback for a
  renamed or missing caveat. The remaining risk, a marker appearing next to a skill
  wrapper in a future version, is measured at 0 for both and would need both a new shape
  and a reading of it as a local command.
- **Veto (amended 2026-11-16, decided by the person after the verification gate
  escalated):** a record carrying `origin.kind: "human"` or any `turnOrigin` is never a
  caveat, an output or a wrapper, whatever its text begins with. Without it, a typed prompt
  that opens with `<local-command-stdout>` would be skipped and read as a false `Waiting`,
  which the first Constraint forbids. Measured 2026-11-15T01:05Z: no caveat or output record
  carries either field (0 of 65), no local wrapper does (0 of 44), and no human-marked record
  begins with a local-command tag, so the veto changes the reading of no measured record. It
  also keeps a skill command a prompt after a stray caveat (39 of 42 skill wrappers carry
  `origin.kind: "human"`). Logs written before 2.1.300 carry neither field, so there the
  text-only rule stands (Known Gaps).

### D2: Local-command records are transparent to the tail classification
- **Status**: assumed
- **Rationale**: `last_meaningful` skips, newest first: every local-command output record
  (D1), every `user` record whose text begins `<local-command-caveat>` (always — its text
  says the command "was run directly in Claude Code, not sent to you"), and a command
  wrapper D1 recognises as local. The classification falls to the record before
  the command, which is what the person last saw happen: after `end_turn` → `/model`, the
  card keeps `Waiting for input`. When nothing meaningful remains in the window (a fresh
  log after `/clear` holds only the caveat, wrapper and output, and D9's wider read finds
  nothing either), `last` is `None` and the app's recency fallback runs — never `Waiting`.
  `TailState.role` is **not** changed by the skip: it stays the raw last parseable line's
  `type`. So the fallback reads Busy for ≤ 10 s when that line is a `system` record (the
  mixed `/clear` form) and ≤ 100 s (`STALE_LIMIT`) when it is a `user` output record, then
  Idle. Rejected: computing `role` past skipped records — `role` feeds only that fallback,
  and changing it widens this spec into the recency rule for one case that only a window of
  nothing but local commands reaches. Rejected: a new
  `LastMessage::LocalCommand` variant mapped to `Waiting` — it would change the app's
  mapping and the overview's buckets, and would claim the human owns the move even when the
  command was typed while a background task is out (D5 keeps that `Working`).

### D3: A wrapper with neither marker is still a prompt
- **Status**: resolved (research)
- **Rationale**: A command wrapper with no caveat before it and no output after it keeps
  today's reading (`UserPrompt` → `Working`). That is every skill command, for good, and a
  local command only if Claude Code stops writing the caveat — then it reads `Working`
  until its output is written (0.04–15.1 s measured; unbounded while a picker stays open)
  and flips then. Every ambiguity in this spec resolves to the pre-feature answer, never
  toward `Waiting`.

### D4: One newest-first filter serves every walk in `activity.rs`
- **Status**: assumed
- **Rationale**: The skip in D2 is one function over records walked newest first, used by
  `last_meaningful`, `newest_is_notification`, `skip_handed_back` and `background_pending`.
  Otherwise D27 (a trailing handed-back notification) would stop composing with a local
  command typed after it, and the background walk would end at the command (D5). Two
  local commands in a row are two pairs; the filter handles each in turn. Rejected:
  patching `last_meaningful` alone — the smallest diff, but it reintroduces exactly the
  "two walks, two definitions" drift `session-overview-ui` D25 removed.

### D5: `is_human_prompt` — local commands no, skill and `!` commands yes
- **Status**: assumed
- **Rationale**: Per record, `is_human_prompt` additionally rejects a local-command output
  record (D1's `user` form; the `system` form is already not `type: "user"`). The caveat
  is already rejected (`isMeta: true`). A wrapper cannot be judged per record (D1 needs its
  neighbours), so the walks drop recognised wrappers through D4's filter before asking, and
  the digest drops caveated ones with D6. Consequence: background work started before a `/model`
  still holds `Working` after it, where the walk used to stop at the command and read
  nothing pending. Skill commands stay prompts (they open turns Claude answers), and so do
  `!` commands (answered, 4 of 4) and continuation summaries. The doc comment's "slash
  commands … all count" is rewritten to say which.

### D6: The digest skips a caveated wrapper; it does not undo
- **Status**: assumed; revised after L2 review
- **Rationale**: `digest_turn` is a forward fold with no look-ahead, so it uses the half of
  D1 it can see: a caveat record arms a flag — a field on `SessionTelemetry`, because
  `apply` folds one line per call and the caveat and wrapper arrive in separate calls,
  possibly with D7-tolerated records between them; `restarted()` (the truncation reset)
  builds a fresh accumulator and so clears it — and if the next `user` /
  `assistant` / `system` record (D7) is a command wrapper, that wrapper is not a prompt —
  no turn is opened, an open turn is not closed, `turn_bg` is kept and `last_prompt_ms` does
  not move. Any other such record disarms the flag. A local-command output record is never
  a prompt (D5), so the mixed form's `system` output and the user form's output open
  nothing either; `apply_claude` calls `digest_turn` for every record, so both reach it.
  Rejected: saving the turn state at every wrapper and restoring it when an output follows
  — it covers only wrappers without a caveat, which measure 0 of 36, at the cost of cloning
  the turn deque and a restore path; without it, a caveat-less local command opens a
  digest turn that the next prompt closes, which is today's behaviour. **Accepted
  divergence:** for such a command the card (D1 via its output) and the digest (caveat
  only) disagree — the card returns to its prior reading once the output lands, while the
  digest's main lane reads working and elapsed restarts until the next prompt. Today both
  read Working, so they agree. Accepted because it is measured 0 of 36 and errs in the
  digest only, which drives no `Waiting`; a renamed caveat would make it every user-form
  command (D10). Rejected: buffering
  the wrapper until the next record — a snapshot in between would miss a skill command's
  turn.

### D7: "Adjacent" means the nearest `user`, `assistant` or `system` record
- **Status**: assumed
- **Rationale**: Measured, the caveat is the record right before the wrapper (36 of 36)
  and the output the record right after it (44 of 44). The rule tolerates records of other
  types in between (`attachment`, `file-history-snapshot`, `queue-operation`,
  `last-prompt`, `ai-title`, …) because those carry no turn meaning and Claude Code
  interleaves them freely elsewhere. A `user`, `assistant` or `system` record of any other
  kind in between breaks the adjacency, and that marker does not count (D3). In the
  newest-first walks the caveat is the wrapper's next *older* record, so the walk looks one
  record ahead in its own direction; when that record lies outside the window (the wrapper
  is the window's oldest complete record), only the output can mark it. The measured
  adjacency is stricter than this rule (line by line, Research), so the tolerance is
  assumed, not observed. `system` records of every subtype (`turn_duration`,
  `stop_hook_summary`, `compact_boundary`, …) break the adjacency; that errs toward
  today's reading. Rejected: strict line adjacency — it would turn an interleaved
  bookkeeping record into a false prompt, the opposite of the rest of this spec's bias.

### D8: `<local-command-stderr>` is recognised beside `<local-command-stdout>`
- **Status**: assumed
- **Rationale**: By construction — 0 records in the corpus. A failing local command is
  still local; recognising the tag costs one prefix and can only turn a `Working` into the
  prior reading for a record that is, by its name, command output. Recorded as unmeasured
  in Known Gaps.

### D9: No new read, no new dependency, byte budget unchanged
- **Status**: resolved (L1)
- **Rationale**: Every walk runs over text it already has: `last_meaningful` and
  `newest_is_notification` over the `TAIL_CAP` tail, `skip_handed_back` and
  `background_pending` over the one shared `PROMPT_SCAN_CAP` window. A real human prompt
  still costs the tail alone (`claude-background-working` R4.7). A window cut through a
  record keeps today's rules: an unreadable line newer than the classified record yields
  `None`; a walk stops at the first unparseable line. **One widening (revised after L2
  review):** when the tail classification comes out `None` *and* the walk skipped at least
  one `user` record under D2 — a caveat, a recognised `user` wrapper or a `user` output
  record; skipping `system` records, which today's walk already passes over, does not
  count — `state_from_reads` classifies again over the shared
  `PROMPT_SCAN_CAP` window (the same one read the two other rules use, read at most once),
  so a run of commands larger than the 8 KiB tail still finds the record before it. When
  that wider classification is used, D27's gate (`newest_is_notification`) reads the same
  wider window through D4's filter too, so a handed-back notification behind such a run is
  still skipped (R2.6). That
  stays inside `TAIL_CAP` + `PROMPT_SCAN_CAP`; a log whose newest message is a real human
  prompt still costs the tail alone, and so does a tail holding nothing but system-form
  command records (its `None` involves no skipped `user` record, so no widening). A tail
  that reaches an `end_turn` still pays the existing back-scan read, as today. A `None`
  from the wider window stands. Rejected:
  widening always on `None` — it would change the cost of every half-written tail, which
  this feature has no reason to touch. Only `crates/ai-monitor` changes.

### D10: The definition is written down where G8 keeps the others
- **Status**: assumed; revised after L2 review
- **Rationale**: `docs/architecture.md` G8's "the human prompt that bounds the turn" bullet
  gains the local-command exception and every string it rests on, each with its rename:
  - `<command-` (the wrapper prefix): renamed → the wrapper is not recognised as a command
    and reads as a prompt, today's `Working`;
  - `<local-command-caveat>`: renamed → user-form wrappers fall back to output pairing, the
    card reads `Working` until the output lands, and the digest opens a turn for every
    user-form command again (closed by the next prompt) — so card and digest then
    **disagree** on every such command (D6's accepted divergence, no longer 0 of 36);
  - `<local-command-stdout>` / `<local-command-stderr>` (user and system output): renamed →
    the tail falls back to the caveat alone, and the output record itself reads as a
    prompt again if it is a `user` record;
  - `system` `subtype: "local_command"`: renamed → only the mixed and system forms lose
    their output marker; the mixed form still has its caveat.

  None of them can produce a false `Waiting`. The oracle: "a card reads Working after
  `/model`" should be read as "grep the log's newest records for `<local-command-caveat>`
  and `<local-command-stdout>`". The `is_human_prompt` doc comment says the same in brief.

### D11: Tests go through the public boundary, with real record shapes
- **Status**: resolved (L1, CLAUDE.md conventions)
- **Rationale**: Acceptance tests drive `ai_monitor::activity::tail_state` and
  `SessionTelemetry::with_digest()` + `apply` + `snapshot()`, with records shaped exactly as
  the Research table (fields copied from real records, paths scrubbed). Unit tests beside
  the code cover the private helpers. Coverage is judged by mutation: `cargo mutants -f
  crates/ai-monitor/src/activity.rs -j 4` scoped, survivor count recorded, a survivor
  treated as a missing test unless provably equivalent.

## Constraints
- Never produce a false `Waiting`. Every case this spec cannot decide keeps the
  pre-feature reading.
- A skill command reads exactly as today in every path (tail, background walk, digest).
- `tail_state`'s byte budget is unchanged: at most `TAIL_CAP` + `PROMPT_SCAN_CAP`, and the
  tail alone for a log whose newest meaningful message is a real human prompt.
- No source or doc change outside `crates/ai-monitor` and `docs/architecture.md`; no new
  dependency; `Cargo.lock` untouched; no `unwrap()` outside tests. Run artefacts under
  `specs/local-command-not-a-prompt/` (the run report) are not source changes.
- `recent_prompts` (tab label) output is unchanged.
- Edits to `.rs` files go through Edit/Write so the rustfmt hook runs.

## Known Gaps
- `/context`'s trailing `Um:## Context Usage` record still reads `UserPrompt` → `Working`
  (2 measured, neither answered). Its only discriminator is its text, and treating an
  `isMeta` record after a local output as part of the command would also swallow an
  answered one (a peer message lands as `isMeta` too). Unchanged from today.
- A manual `/compact` writes its continuation summary *before* the command, so after the
  command is skipped the summary reads `UserPrompt` → `Working`. Unchanged from today; the
  summary is answered when compaction is automatic, so it cannot be skipped by type.
- A local command whose wrapper has no caveat (0 of 36 measured) reads `Working` in the
  card until its output lands (unbounded for an open picker), and opens a digest turn that
  the next prompt closes (D3, D6). After the output lands, card and digest **disagree** on
  it (D6's accepted divergence); today they agree on Working.
- In the newest-first walks, a wrapper that is the oldest complete record in its window
  cannot see its caveat and relies on its output (D7).
- `<local-command-stderr>` is by construction (D8), 0 corpus records.
- The corpus counts are a reading at 2026-11-14T23:51Z; Claude Code may change the shapes
  (G8's degradation note, D10).
- L2 review ended NEEDS_FIX after its 2 retries. Its last four issues (D2's recency claim
  and `role`; Done-when checks for D4/D5; which skips trigger D9's wider read; D6's flag as
  accumulator state) were each addressed in the decisions above but not re-reviewed at L2;
  the L4 Spec Review re-checks the whole spec.
- D1's veto relies on `origin` / `turnOrigin`, which Claude Code writes from 2.1.300 on. In an
  older log, a typed prompt that opens with a local-command tag is still read by its text
  alone (0 such prompts measured).
- The earlier message the request refers to was not available to this run; its content
  was reconstructed (Confirmed Goal, inferred items).

## Requirements

Record shorthand used below (shapes exactly as the Research table; every record also carries
`timestamp`, `sessionId`, `uuid` like a real one):
- **caveat** — `{"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat: The messages below were generated by the user while running local commands. …</local-command-caveat>"}}`
- **U-wrapper(/x)** — `{"type":"user","message":{"role":"user","content":"<command-name>/x</command-name>\n            <command-message>x</command-message>\n            <command-args></command-args>"}}`
- **U-output** — `{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to …</local-command-stdout>"}}`
- **S-output** — `{"type":"system","subtype":"local_command","content":"<local-command-stdout></local-command-stdout>","level":"info","isMeta":false,"commandRun":{"command":"clear","args":""}}`
- **S-wrapper(/x)** — `{"type":"system","subtype":"local_command","content":"<command-name>/x</command-name>…","level":"info","isMeta":false}`
- **skill(/s)** — `{"type":"user","origin":{"kind":"human"},"message":{"role":"user","content":"<command-message>s</command-message>\n<command-name>/s</command-name>"}}` then `{"type":"user","isMeta":true,"message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /x\n\n# …"}]}}`
- **end_turn** — an assistant record with `message.stop_reason: "end_turn"`.

### R0: A local command leaves a session's activity reading as it was — D1–D11

#### R0.1: The card, the background hold and the digest all ignore a local command (manual)
- **Given**: the implementation of R1–R4 is merged
- **When**: a reviewer reads `crates/ai-monitor/src/activity.rs` and `telemetry.rs`
- **Then**: one set of recognisers (caveat, wrapper, output) is used by every walk and by
  `digest_turn`; no other source or doc file changed except `docs/architecture.md` and new
  tests

#### R0.2: G8 documents the definition and every string it rests on (manual)
- **Given**: `docs/architecture.md` G8's "the human prompt that bounds the turn" bullet
- **When**: a reader looks up why a card reads Working after `/model`
- **Then**: the bullet names the local-command exception, the strings `<command-`,
  `<local-command-caveat>`, `<local-command-stdout>`, `<local-command-stderr>` and
  `system` `subtype: "local_command"`, how each rename degrades (D10, including D6's card/
  digest divergence), and the oracle line

#### R0.3: Mutation coverage is recorded (manual)
- **Given**: the change is complete
- **When**: `cargo mutants -f crates/ai-monitor/src/activity.rs -j 4` and the same for
  `crates/ai-monitor/src/telemetry.rs` (scoped with `--re` to the changed functions if the
  full file is too slow) are run
- **Then**: the survivor count in the changed functions is recorded in the run report
  (`specs/local-command-not-a-prompt/unattended-report.md`, which the executing run writes),
  and each survivor is either killed by a new test or argued equivalent

### R1: The tail classification skips a local command — D1, D2, D3, D7, D8

#### R1.1: User form after end_turn reads the end_turn
- **Given**: a log `human prompt → end_turn → caveat → U-wrapper(/effort) → U-output`
- **When**: `ai_monitor::activity::tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)` (today `UserPrompt`)

#### R1.2: Mixed form after end_turn reads the end_turn
- **Given**: a log `human prompt → end_turn → caveat → U-wrapper(/clear) → S-output`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R1.3: Pure system form after end_turn is unchanged (regression guard)
- **Given**: a log `human prompt → end_turn → S-wrapper(/model) → S-output`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R1.4: A skill command is still a prompt
- **Given**: a log `human prompt → end_turn → skill(/s)` (wrapper + isMeta skill body, no
  caveat, no output)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::UserPrompt)`

#### R1.5: The caveat alone marks the wrapper, before the output is written
- **Given**: a log `human prompt → end_turn → caveat → U-wrapper(/model)` (no output yet)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R1.6: The output alone marks the wrapper when no caveat precedes it
- **Given**: a log `human prompt → end_turn → U-wrapper(/model) → U-output` (no caveat)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R1.7: A wrapper with neither marker is a prompt
- **Given**: a log `human prompt → end_turn → U-wrapper(/model)` (no caveat, no output)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::UserPrompt)`

#### R1.8: Non-turn records between the markers and the wrapper are tolerated
- **Given**: a log `human prompt → end_turn → caveat → attachment → U-wrapper(/model) →
  file-history-snapshot → U-output`, where `attachment` is `{"type":"attachment",…}` and
  `file-history-snapshot` is `{"type":"file-history-snapshot",…}`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R1.9: A system record between breaks the adjacency
- **Given**: a log `human prompt → end_turn → U-wrapper(/model) →
  {"type":"system","subtype":"turn_duration",…} → U-output` (no caveat)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::UserPrompt)` — the wrapper is unmarked and the
  output record itself is skipped

#### R1.10: A stderr output marks the wrapper
- **Given**: a log `human prompt → end_turn → U-wrapper(/model) →
  {"type":"user","message":{"role":"user","content":"<local-command-stderr>boom</local-command-stderr>"}}`
  (no caveat)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R1.11: A local command typed mid-turn reads the busy turn
- **Given**: a log `human prompt → assistant tool_use (stop_reason "tool_use", tool Bash)
  → caveat → U-wrapper(/model) → U-output`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantBusy)`

#### R1.12: Two local commands in a row
- **Given**: a log `human prompt → end_turn → caveat → U-wrapper(/model) → U-output →
  caveat → U-wrapper(/effort) → U-output`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R1.13: A half-written line after a local command still reads None
- **Given**: a log `human prompt → end_turn → caveat → U-wrapper(/model) → U-output →
  {"type":"assistant","mess` (no trailing newline)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == None`

#### R1.14: A fresh /clear log reads None with role "system"
- **Given**: a log holding only `caveat → U-wrapper(/clear) → S-output`
- **When**: `tail_state(&log)` is called
- **Then**: it returns `Some(state)` with `state.last == None` and `state.role == "system"`

#### R1.15: A typed prompt that opens with a local-command tag is a prompt (D1 veto)
- **Given**: a log `end_turn → {"type":"user","origin":{"kind":"human"},"message":{"role":"user","content":"<local-command-stdout>Set model to opus</local-command-stdout> why did it pick this?"}}`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::UserPrompt)` (added after the gate; tested in
  `activity.rs`'s `mod tests`, `a_typed_prompt_opening_with_a_local_command_tag_reads_as_a_prompt`)

### R2: The background walk and D27 see through a local command — D4, D5, D9

Background start = a `user` record with `toolUseResult: {"backgroundTaskId": "b1"}` and a
`tool_result` block; completion = a `queue-operation` `enqueue` whose `content` begins
`<task-notification> <task-id>b1</task-id>` (G8's shapes, as
`crates/ai-monitor/tests/tt1_claude_background.rs` builds them).

#### R2.1: Pending background work survives a local command
- **Given**: a log `human prompt → assistant tool_use Bash → background start(b1) →
  end_turn → caveat → U-wrapper(/model) → U-output`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantBusy)` (today `UserPrompt`)

#### R2.2: Finished background work is still finished across a local command
- **Given**: R2.1's log with the completion of `b1` inserted before the caveat
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R2.3: A skill command still bounds the walk
- **Given**: a log `human prompt → assistant tool_use Bash → background start(b1) →
  end_turn → skill(/s) → end_turn`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)` — the walk stops at the skill
  command's wrapper, so `b1` from the earlier turn is not counted

#### R2.4: A `!` command still bounds the walk
- **Given**: a log `human prompt → assistant tool_use Bash → background start(b1) →
  end_turn → {"type":"user","message":{"role":"user","content":"<bash-input>ls</bash-input>"}}
  → {"type":"user","message":{"role":"user","content":"<bash-stdout>a</bash-stdout><bash-stderr></bash-stderr>"}}
  → end_turn`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R2.5: A handed-back notification is skipped through a local command
- **Given**: a log `human prompt → assistant tool_use Agent → background Agent launch
  (agentId "a1", isAsync, async_launched) → {"type":"user","isMeta":true,"origin":{"kind":"peer","from":"a1","handback":true},…} →
  end_turn → {"type":"user","origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification> <task-id>a1</task-id> <status>completed</status> </task-notification>"}}
  → caveat → U-wrapper(/effort) → U-output` (the shape of
  `crates/ai-monitor/tests/tt5_claude_real_shapes.rs` and `activity.rs`'s D27 tests)
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)` (without the local command this
  log already reads `AssistantEndTurn`; with it, today it reads `UserPrompt`)

#### R2.6: A handed-back notification behind a run of commands larger than the tail
- **Given**: R2.5's log, with the single `caveat → U-wrapper(/effort) → U-output` group
  replaced by enough groups (U-output text padded to ~3 KiB) to exceed
  `ai_monitor::activity::TAIL_CAP`
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

### R3: A run of local commands larger than the tail still finds the record before it — D9

#### R3.1: Wider read on a skipped-only tail
- **Given**: a log `human prompt → end_turn` followed by enough
  `caveat → U-wrapper(/model) → U-output` groups (each U-output's text padded to ~3 KiB, the
  measured maximum) that the groups exceed `ai_monitor::activity::TAIL_CAP` bytes
- **When**: `tail_state(&log)` is called
- **Then**: `.last == Some(LastMessage::AssistantEndTurn)`

#### R3.2: The byte budget holds (manual)
- **Given**: `activity::state_from_reads` driven through its `read_last` closure, which
  records every requested cap (crate-internal, so the check lives in `activity.rs`'s
  `mod tests`, the way `claude-background-working` R4.7 is checked)
- **When**: it classifies (a) a tail whose newest meaningful message is a human prompt,
  (b) a tail holding nothing but system-form command records (`S-wrapper` / `S-output`
  pairs, no `end_turn` and no other meaningful record in the tail), and (c) a tail of only
  skipped `user` local-command records
- **Then**: (a) and (b) request `TAIL_CAP` only; (c) requests `TAIL_CAP` then
  `PROMPT_SCAN_CAP` once, and no more even when the wider classification is `end_turn` and
  `background_pending` runs

### R4: The digest neither opens nor closes a turn for a caveated local command — D5, D6

Each record below is passed as its own `SessionTelemetry::apply(line)` call on an accumulator
built `SessionTelemetry::new().with_digest()`; the result is read from `snapshot()`.

#### R4.1: A local command after a finished turn moves nothing
- **Given**: `human prompt (t0) → end_turn (t1)` applied
- **When**: `caveat (t2)`, `U-wrapper(/model) (t2)`, `U-output (t2)` are applied
- **Then**: `snapshot().last_prompt_ms == Some(t0)`, `snapshot().turns.len() == 1`, and
  that turn's `end_ms == Some(t1)`

#### R4.2: A local command during an open turn leaves it open
- **Given**: `human prompt (t0) → assistant tool_use (t1)` applied (no `end_turn`)
- **When**: `caveat`, `U-wrapper(/model)`, `U-output` are applied
- **Then**: `snapshot().turns.len() == 1`, its `end_ms == None`, and
  `last_prompt_ms == Some(t0)`

#### R4.3: Interleaved records do not disarm the caveat
- **Given**: `human prompt (t0) → end_turn (t1)` applied
- **When**: `caveat`, an `{"type":"attachment",…}` record, `U-wrapper(/model)`,
  `U-output` are applied, each in its own call
- **Then**: `last_prompt_ms == Some(t0)` and `turns.len() == 1`

#### R4.4: A skill command opens a turn
- **Given**: `human prompt (t0) → end_turn (t1)` applied
- **When**: `skill(/s)` (wrapper at t2, then the skill body) is applied
- **Then**: `last_prompt_ms == Some(t2)` and `turns.len() == 2` with the front turn's
  `end_ms == None`

#### R4.5: A caveat followed by something other than a wrapper disarms
- **Given**: `human prompt (t0) → end_turn (t1) → caveat (t2)` applied
- **When**: a human prompt at t3 (`origin.kind: "human"`, plain text) is applied
- **Then**: `last_prompt_ms == Some(t3)` and `turns.len() == 2`

#### R4.6: A wrapper with no caveat still opens a turn (accepted divergence)
- **Given**: `human prompt (t0) → end_turn (t1)` applied
- **When**: `U-wrapper(/model) (t2)` and `U-output (t2)` are applied with no caveat
- **Then**: `last_prompt_ms == Some(t2)` and `turns.len() == 2` — and the `U-output` itself
  opens no further turn (`turns.len()` stays 2)

## Tasks

No task is fenced: nothing here touches a database schema, a data-losing migration,
auth / access control, payment or security.

### TT1: acceptance tests for R1's GWTs [test]
- **Fulfills**: R1 (tests)
- **Depends on**: (none)
- **Files**: creates `crates/ai-monitor/tests/tt1_local_command_tail.rs` only.
- **Notes**: One test per R1.n, named `r1_n_…`. Each writes its log to a per-test file
  under `std::env::temp_dir()` (pattern of `tt5_claude_real_shapes.rs`), records built with
  `serde_json::json!` in the Requirements' shorthand shapes, and asserts on
  `ai_monitor::activity::tail_state`. Written before T1, so R1.1, R1.2, R1.5, R1.6, R1.8,
  R1.10, R1.11, R1.12 and R1.14 fail on today's code (they read `UserPrompt`), and R1.3,
  R1.4, R1.7, R1.9 and R1.13 pass already (guards).

### TT2: acceptance tests for R2's GWTs [test]
- **Fulfills**: R2 (tests)
- **Depends on**: (none) — parallel with TT1, TT3, TT4
- **Files**: creates `crates/ai-monitor/tests/tt2_local_command_walks.rs` only.
- **Notes**: Same harness as TT1. R2.1, R2.2, R2.5 and R2.6 fail on today's code (they read
  `UserPrompt`); R2.3 and R2.4 are guards. R2.6 asserts its groups exceed `TAIL_CAP` before
  asserting the classification.

### TT3: acceptance tests for R3's GWTs [test]
- **Fulfills**: R3 (tests)
- **Depends on**: (none) — parallel with TT1, TT2, TT4
- **Files**: creates `crates/ai-monitor/tests/tt3_local_command_wide_tail.rs` only.
- **Notes**: R3.1 only; it asserts the log's local-command groups really exceed
  `ai_monitor::activity::TAIL_CAP` before asserting the classification, so the test
  cannot pass by fitting in the tail. R3.2 is `(manual)` at this boundary and is checked
  by T1's unit test.

### TT4: acceptance tests for R4's GWTs [test]
- **Fulfills**: R4 (tests)
- **Depends on**: (none) — parallel with TT1, TT2, TT3
- **Files**: creates `crates/ai-monitor/tests/tt4_local_command_digest.rs` only.
- **Notes**: Uses `ai_monitor::telemetry::SessionTelemetry::new().with_digest()`, one
  `apply(&line)` per record, `snapshot()`'s `last_prompt_ms` and `turns` (pattern of
  `tt8_digest_timeline.rs`, including its `stamp` / `ms` helpers). R4.1–R4.3 fail on
  today's code, and so does R4.6's second half (today the `U-output` opens a third turn);
  R4.4 and R4.5 are guards.

### T1: Recognise local commands in every `activity.rs` walk [vertical]
- **Fulfills**: R1, R2, R3, R0
- **Depends on**: TT1, TT2, TT3
- **Files**: `crates/ai-monitor/src/activity.rs` only.
- **Work**:
  1. Add the recognisers (D1, D8): `is_local_caveat(v)` — `type: "user"` and message text
     (`message_str`) beginning `<local-command-caveat>` after leading whitespace;
     `is_local_output(v)` — a `user` record whose text begins `<local-command-stdout>` or
     `<local-command-stderr>`, or a `system` record with `subtype: "local_command"` whose
     `content` string begins with either; `is_user_wrapper(v)` — `type: "user"`, no
     `toolUseResult`, text beginning `<command-`. `is_local_caveat` and `is_user_wrapper`
     are `pub(crate)` for T2.
  2. Add one newest-first filter (D4, D7) over a `&[Value]` already ordered newest first,
     yielding the records that remain once local-command records are dropped: every
     `is_local_output` and `is_local_caveat` record, and every `is_user_wrapper` record whose
     nearest newer `user`/`assistant`/`system` record is an output, or whose nearest older
     one is a caveat. Records of any other `type` are passed through and never count as
     "nearest". It also reports whether it dropped any `user` record (for D9).
  3. Route `last_meaningful`, `newest_is_notification`, `skip_handed_back` and
     `background_pending` through it (D2, D4). `last_meaningful` keeps its unreadable-line
     rule: it splits the window into lines newest first, stops collecting at the first
     unparseable line and remembers whether one sat newer than the record it returns
     (exactly today's `None` cases, R1.13).
  4. `is_human_prompt` additionally rejects `is_local_output` records (D5); rewrite its doc
     comment's "slash commands … all count" sentence to say local commands do not, skill
     and `!` commands and continuation summaries still do, with the D10 oracle in one line.
  5. `state_from_reads` (D9): when `last_meaningful` returns `None` and the filter dropped a
     `user` record, classify again over the shared `scan` window (the existing closure, so
     the `PROMPT_SCAN_CAP` read happens at most once), then let the D27 and background rules
     run on that result; when the wider classification was used, D27's
     `newest_is_notification` gate reads that same window (through the filter) instead of
     the tail (R2.6). `TailState.role` is unchanged.
  6. Unit tests in `activity.rs`'s `mod tests`: the recognisers' boundaries (leading
     whitespace, array vs string content, a `system` record of another subtype, a quoted
     `<local-command-stdout>` mid-text not matching) and R3.2's byte budget, recording the
     caps passed to `state_from_reads`' closure.
  7. `cargo test -p ai-monitor`, `cargo clippy --workspace --all-targets -- -D warnings`;
     then `cargo mutants -f crates/ai-monitor/src/activity.rs -j 4` (scope with `--re` to the
     changed functions if needed) and record the survivor count in the run report (R0.3).

### T2: The digest skips a caveated wrapper [vertical]
- **Fulfills**: R4, R0
- **Depends on**: T1, TT4
- **Files**: `crates/ai-monitor/src/telemetry.rs` only.
- **Work**:
  1. Add a field to `SessionTelemetry` (beside `turn_bg` / `turn_ended`), e.g.
     `local_caveat: bool` (D6). In `digest_turn` (Claude path only; `ag_digest_turn` is
     untouched): an `activity::is_local_caveat` record sets it and returns; on any other
     record whose `type` is `user`, `assistant` or `system`, read and clear it — if it was set
     and the record is `activity::is_user_wrapper`, return before the prompt branch. Records
     of other types leave it alone (R4.3). `restarted()` already builds a fresh accumulator,
     so nothing else resets it.
  2. A `<local-command-stdout>` user record no longer opens a turn because T1's
     `is_human_prompt` rejects it (R4.6's second half); no code here for it.
  3. Unit test in `telemetry.rs`'s tests that `restarted()` clears the armed flag (a caveat
     before the reset, a wrapper after it opens a turn).
  4. `cargo test -p ai-monitor`, clippy as above; `cargo mutants -f
     crates/ai-monitor/src/telemetry.rs -j 4 --re digest_turn` and record survivors in the
     run report (R0.3).

### T3: Document the definition in G8 [vertical]
- **Fulfills**: R0
- **Depends on**: T1, T2
- **Files**: `docs/architecture.md` only.
- **Work**: Extend G8's Claude background-work bullet "the human prompt that bounds the
  turn" (`docs/architecture.md:300`) with a sub-bullet per D10: the local-command exception
  (caveat before or output after a `user` wrapper), each string and its rename, D6's
  accepted card/digest divergence, the two unchanged gaps (`/context`'s trailing record,
  manual `/compact`), and the oracle sentence in G8's usual bold "should be read as" form.

## External Dependencies

### Pre-work
(none)

### Post-work
- (person) Clear the tmux server's leaked environment, as the request said:
  `tmux set-environment -gu CLAUDECODE`, `… -gu CLAUDE_CODE_CHILD_SESSION`,
  `… -gu TERM_PROGRAM` and the stale socket variable, or restart the tmux server once the
  other sessions finish. Not code; listed so it is not lost.
- (person, optional) Eyeball the fix: in a Crewterm tab run `claude`, let a turn end, type
  `/model` and pick the current model; the card should keep `Waiting for input`.

## Spec Review
- **Verdict**: PASS
- **Reviews**: 3
- **Fixed**:
  - [Testability / Consistency] R3.2 (b), D9 — case (b) now has no `end_turn` in the tail; the
    existing back-scan read after an `end_turn` is named as unchanged.
  - [Consistency] R0.3, R0.1, Constraints, T1, T2 — the mutation survivor count goes to the
    run report; run artefacts under `specs/local-command-not-a-prompt/` are exempt from the
    "no other file" rules.
  - [Readiness] T1 step 5, D9 — after a wider read, D27's `newest_is_notification` gate reads
    the same wider window; R2.6 added, and TT2 covers it.
- **Open**: (none)
