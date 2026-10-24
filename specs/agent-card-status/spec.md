# Spec: agent-card-status

## Meta
- **Created**: 2026-09-28
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-09-28
- **Branch**: feat/improve-card-ui (on top of `specs/sidebar-weather-cards`)

## Goal
Sidebar card agent status for non-Claude agents and permission prompts: the tabmeta worker
returns `AgentState::Unknown` for every non-Claude agent (`crates/app/src/tabmeta/worker.rs:211`),
so an Antigravity/Codex/Gemini/Aider card always shows "Working" (sidebar-weather-cards D7 maps
Unknown → Working), even when the agent is blocked on a tool-permission prompt. Make the worker
derive real state for Antigravity (whose session the ai-monitor panel already reads) and detect
a pending tool-permission prompt as Waiting for input — for Claude too, where `activity.rs` only
treats end_turn / AskUserQuestion as Waiting and a Bash permission prompt reads Busy. Observed:
an Antigravity tab waiting on "Requesting permission for: git log -n 5 --oneline" showed
"Working".

## Non-goals
- The card's look: skins, icons, sizes, colours and zones stay as `sidebar-weather-cards`
  built them. Only which status a card shows changes, plus hiding it for agents with no
  readable state.
- The ai-monitor right panel.
- Agent detection itself (which process is which agent): `specs/antigravity-agy-detection`,
  `specs/agent-classifier-deleted-binary`.
- Real state for Codex, Gemini and Aider. No session reader exists for them; building one is
  a separate feature.

## Confirmed Goal
Make an agent tab's card status true:

- **Permission dialogs.** When Claude Code **or** the Antigravity CLI is blocked on a
  tool-permission dialog, the card reads **Waiting for input** and the tab appears in the
  waiting band. The on-screen detector (`crates/app/src/prompt.rs`,
  `specs/agent-prompt-waiting`) learns Antigravity's dialog — measured from the real
  "Requesting permission for: … / Run this command?" screen — and its result reaches the
  card, not only the band.
- **Antigravity's own state.** An Antigravity card shows **Working / Ready / Waiting for
  input** derived from its session transcript, the way a Claude card does from its log,
  instead of the constant `Unknown` → "Working".
- **Agents with no readable state** (Codex, Gemini, Aider): the card **hides** the status
  zone rather than claiming "Working"; the branch still shows. (User decision at the
  mirror.)

Inferred beyond the literal request: the waiting band and the card must not contradict
each other — a tab the band lists as waiting on a dialog cannot have a card that reads
"Working". Today they can: the prompt detector feeds only the band, while the card reads
the log-derived `AgentState`.

Done when: an Antigravity tab on a permission dialog shows "Waiting for input" on its card
and a band row; answering returns it to "Working"; an Antigravity session whose turn is over
reads "Waiting for input", as a Claude one does (L2 D2 — the mirror's first draft said
"Ready", corrected at L2); a Claude tab on a permission dialog shows "Waiting for input" on
its card; a card whose agent state is unknown — always for Codex/Gemini/Aider — shows no
status; `cargo build`, `cargo test --workspace` and
`cargo clippy --workspace --all-targets -- -D warnings` are green; and `cargo mutants` on
the changed files leaves no survivor that is not argued equivalent.

## Research

### Where a card's status comes from today
- The card draws `RowMeta::status`, resolved app-side by `sidebar_model::resolve_status(agent,
  running, last_exit)`: Busy/Unknown → `Working`, Idle → `Ready`, Waiting → `WaitingForInput`,
  then running → `Running`, a reportable exit → `Exited`, else `Shell`
  (`crates/app/src/sidebar_model.rs`, `sidebar-weather-cards` D7). `agent` is
  `TabMeta::agent` from the tabmeta worker's snapshot.
- The worker derives `AgentState` only for Claude: `worker.rs:211-212` returns
  `(AgentState::Unknown, None)` for every other `AgentKind`, and only Claude's
  `activity::resolve_log` is called (`worker.rs:230`). Pinned by the test
  `only_a_claude_kind_agent_is_published` (`worker.rs:1321`, Antigravity at `:1338-1346`).
- Claude's rule (`crates/app/src/tabmeta/activity.rs:66-90`, `state_from`): `AssistantEndTurn`
  or `AssistantAsksUser` → `Waiting`; `AssistantBusy` / `UserToolResult` / `UserPrompt` →
  `Busy`; no robust last signal → recency fallback (`BUSY_WINDOW` 10 s, `STALE_LIMIT` 100 s,
  `:44-47`) that **never** yields `Waiting` ("a false 'waiting' is the worst failure").
  `AssistantAsksUser` is a closed list of human-blocking tools (`AskUserQuestion`,
  `ExitPlanMode`) — `specs/agent-asks-user-waiting`.
- `LastMessage` and `tail_state` live in `crates/ai-monitor/src/activity.rs:32, :67`; the tail
  read is capped at 8 KiB (`TAIL_CAP`, `:19`) and goes through Claude's `parse_line`
  (`crates/ai-monitor/src/lib.rs:715`, keyed on `type` user/assistant), so it cannot
  classify an Antigravity transcript.

### The permission-dialog detector (on-screen)
- `crates/app/src/prompt.rs:45` `shows_blocking_prompt(screen)` reads the bottom 20 content
  rows (`TAIL_ROWS`, `:32`) and needs 2 of 3 markers (`REQUIRED_MARKERS`, `:38`): the question
  `Do you want to proceed?` / `Do you want to make this edit?` (`:69-72`), a footer containing
  both `Esc to cancel` and `to amend` (`:76-78`), and a selected choice `❯ N.` (`:82-93`).
  All three are **Claude Code's** wording; the fixture is a real captured Claude dialog
  (`:101`, `real_permission_dialog_is_detected`). `specs/agent-prompt-waiting`.
- It runs from `terminal_app.rs` `push_agent_states` on `PtyOutput` (gated on `advanced`) for
  every tab whose `TabMeta::agent` is `Some` — any agent kind — and only on the live bottom
  (`view_offset() == 0`), via `Terminal::viewport_tail_text` (no grid clone)
  (`terminal_app.rs:1135-1232`).
- Its result goes **only to the waiting band**: a detected tab's state becomes `Waiting` in
  the list handed to `waiting.on_agent_states` (`terminal_app.rs:1170-1180`). The card still
  reads the log-derived `TabMeta::agent.state`, so a Claude tab on a permission dialog can
  be in the band while its card reads `Working`.
- Observed: the Antigravity tab on "Requesting permission for: git log -n 5 --oneline /
  Run this command?" was **not** in the band — none of the three markers match
  Antigravity's wording (question `Run this command?`, footer split over two lines as
  `↑/↓ Navigate · tab Amend · ctrl+g edit/expand command` and `esc to cancel`).

### Antigravity's dialog vocabulary (from the installed `agy` binary, `strings`)
- Questions: `Run this command?`, `Allow access to this URL?`, `Allow calling this tool?`
  (listed together in the binary as examples), `Do you want to proceed?`, `%s requires
  permission to read, edit, and execute files here.`
- Choices: `Yes, run command`, `Yes, allow access`, `Yes, allow creation`, `Yes, accept this
  change`, `No, deny permission`, `No, deny creation`, `Yes, and always allow '%s' in this
  conversation`, `Yes, and always allow '%s' (Persist to settings.json)`, `No, and always deny
  for …`.
- Footer fragments: `esc to cancel` (lower-case, its own line), `edit/expand command`.
- Not found as a literal: `Requesting permission for:` (assembled at runtime); the selected
  choice's pointer glyph is not recoverable from a screenshot (it renders as `>`).
  **Unmeasured** — a real capture is required (see Known Gaps).

### Antigravity's session transcript (measured on this machine, 78 transcripts)
- Path: `~/.gemini/antigravity-cli/brain/<conversation-id>/.system_generated/logs/transcript.jsonl`
  (`crates/ai-monitor/src/antigravity.rs:99`). One JSON step per line: `step_index`,
  `source`, `type`, `status`, `created_at`, and `content` / `tool_calls` / `thinking` /
  `exit_code` / `error` / `truncated_fields`.
- Step kinds (3045 steps): `PLANNER_RESPONSE` (MODEL) — with `tool_calls` (1333) it is a tool
  dispatch, without `tool_calls` it is the model's reply (111: 110 with `content`, 1 empty); tool results `GENERIC`,
  `RUN_COMMAND`, `VIEW_FILE`, `LIST_DIRECTORY`, `GREP_SEARCH`, `CODE_ACTION`, `FIND`,
  `INVOKE_SUBAGENT`, `ASK_QUESTION`; `USER_INPUT` (source `USER_EXPLICIT`); `CHECKPOINT`,
  `SYSTEM_MESSAGE`, `CONVERSATION_HISTORY`, `ERROR_MESSAGE` (SYSTEM).
- `status`: `DONE` 3016, `RUNNING` 28, `ERROR` 1. `RUNNING` is **not** a pending prompt: it
  marks a tool running as a background task ("Tool is running as a background task with task
  id …"). No `step_index` is ever written twice (0 duplicates) — a step is appended once.
- A step is appended **when it completes**: `ASK_QUESTION` records carry the answer ("A1:
  User Skipped") with `Created At` = `Completed At`. So a pending question is not in the
  log as `ASK_QUESTION`; what is there is the preceding `PLANNER_RESPONSE` whose
  `tool_calls` name `ask_question` (11 of the 12 `ASK_QUESTION` steps are preceded by
  exactly that).
- The observed permission wait: step 15 (`PLANNER_RESPONSE`, `tool_calls: run_command`, git
  log) at 07:19:00Z, its result (`GENERIC`) at 07:19:19Z after approval; the ai-monitor panel
  in the screenshot, taken during the dialog, showed "Last update 00:19:00" (PDT) — the
  dispatch was on disk while the dialog was up. From the log alone, "tool dispatched and
  running" and "tool dispatched and awaiting permission" are **indistinguishable**.
- Tool names dispatched (all transcripts): `run_command` 646, `view_file` 357, `list_dir`
  116, `grep_search` 57, `manage_task` 33, `write_to_file` 29, `find_by_name` 24,
  `ask_question` 22, `send_message` 18, `define_subagent` 13, `manage_subagents` 13,
  `replace_file_content` 8, `invoke_subagent` 6, `schedule` 2.
- Last step per transcript: tool-less `PLANNER_RESPONSE` 60 (turn over), `PLANNER_RESPONSE`
  with `tool_calls` 12 (`run_command` 11, `replace_file_content` 1), `CHECKPOINT` 6.

### Which transcript belongs to a tab (exists, but in the panel, not in tabmeta)
- The app answers "which conversation does agy pid P hold" by listing P's open FDs for a
  presence lock: `crates/app/src/presence_fd.rs:75` `conversation_of_pid(pid)` (`proc_pidinfo`
  `PROC_PIDLISTFDS` + `PROC_PIDFDVNODEPATHINFO`, `:117`, `:164`).
  `ai_monitor::antigravity::resolve(Probed, cwd)` (`antigravity.rs:255`, `Probed` at `:266`:
  `Conversation(id)` / `LiveButUnnamed` / `NoAgent`) turns that into a transcript; ai-monitor
  deliberately does not own the FD question (`antigravity.rs:8-11`, headless purity, AG11).
- The only caller is the ai-monitor panel (`crates/app/src/overlay.rs:598-625`,
  `antigravity_pid` at `:1388`). The tabmeta worker never resolves an Antigravity session.
- `crates/ai-monitor/src/telemetry.rs:725` `apply_antigravity` folds a transcript into panel
  telemetry (turns, tool dispatches, `AG_RESULT_STEPS` at `:1649`); it has **no** busy/idle/
  waiting classification — only `last_event_ms` (the panel's IDLE). `antigravity.rs:329`
  `last_step_in_tail` reads an 8 KiB tail for the newest `created_at`.
- Fixture: `crates/ai-monitor/tests/fixtures/antigravity_transcript.jsonl` (17 steps,
  sanitized paths, real format); tests in `crates/ai-monitor/tests/telemetry.rs:2097`,
  `crates/ai-monitor/tests/antigravity.rs`.

### The renderer side
- `renderer::sidebar_card::CardStatus` has `Working`, `Ready`, `WaitingForInput`, `Running`,
  `Exited(i32)`, `Shell`; there is no "no status" value. The status zone is hidden only by the
  global `show_agent_status` toggle (`CardInput::show_status`, `sidebar-weather-cards` D13),
  after which the branch right-aligns from the padding box's left (`sidebar_card.rs`
  `build_card`).

### Constraints inherited
- `term-core` stays policy-free and GPU-free (C4); `prompt.rs` is app-side and reads raw
  viewport text only (`specs/agent-prompt-waiting` D21).
- G1: no unconditional periodic wake; the prompt scan is per PTY chunk and must stay cheap.
- Conservative bias: a false "waiting" is the worst failure; fallbacks never yield Waiting.
- Build/test/lint: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace
  --all-targets -- -D warnings`; coverage by `cargo mutants` (CLAUDE.md).

## Decisions

### D1: Antigravity's state comes from the last meaningful step of its transcript
- **Status**: resolved (auto, from L1)
- **Decision**: the worker reads the tail of the tab's Antigravity transcript and classifies
  its **last meaningful step**, skipping `CHECKPOINT`, `SYSTEM_MESSAGE`,
  `CONVERSATION_HISTORY` and `ERROR_MESSAGE` (bookkeeping the CLI writes around a turn):

  | Last meaningful step | State |
  |---|---|
  | `PLANNER_RESPONSE` without `tool_calls` (the model's reply), no background work since the last `USER_INPUT` | `Waiting` (D2) |
  | the same reply **after** background work since the last `USER_INPUT` (D13), or with that `USER_INPUT` not found in the read window (D14) | no robust signal → recency fallback, never `Waiting` |
  | `PLANNER_RESPONSE` whose `tool_calls` include a human-blocking tool (closed list: `ask_question`) | `Waiting` |
  | `PLANNER_RESPONSE` with any other `tool_calls` (a dispatched tool) | `Busy` |
  | a tool result — the closed set `GENERIC`, `RUN_COMMAND`, `VIEW_FILE`, `LIST_DIRECTORY`, `GREP_SEARCH`, `CODE_ACTION`, `FIND`, `INVOKE_SUBAGENT`, `ASK_QUESTION` (`RUNNING` background tasks included) | `Busy` |
  | `USER_INPUT` | `Busy` |
  | any other step kind (not listed here, not bookkeeping) | no robust signal → recency fallback |
  | none parseable in the tail (e.g. a last line longer than the 8 KiB tail) | Claude's recency fallback (`Busy` within 10 s of the transcript's last write — its `mtime` — else `Idle` → `Ready`; past `STALE_LIMIT` it stays `Idle`) — never `Waiting`, never `Unknown` |

- **Rationale**: this is Claude's rule (`state_from`) transposed onto Antigravity's step
  kinds, so the two agents mean the same thing by each state. Measured on 78 real
  transcripts: steps are appended once, on completion, so a pending `ask_question` is visible
  only as the dispatching `PLANNER_RESPONSE`, and a pending permission is visible only as a
  dispatched tool with no result — which is why permission prompts come from the screen
  (D4), not from here. The closed list mirrors `specs/agent-asks-user-waiting`'s
  `AskUserQuestion`/`ExitPlanMode`.
  - **Rejected: reading `status`.** `RUNNING` marks a background task, not a wait; every
    other step is `DONE`.
  - **Rejected: reusing `ai_monitor::activity::tail_state`.** It parses Claude's record shape
    (`type` user/assistant) and cannot classify an Antigravity step.
  - **Rejected: the panel's `last_event_ms` alone** (IDLE). Time since the last step cannot
    tell a long-running command from a finished turn.

### D2: An Antigravity turn that is over reads Waiting for input, as Claude's does
- **Status**: resolved (user)
- **Decision**: a tool-less `PLANNER_RESPONSE` as the last meaningful step is `Waiting`: the
  card reads **Waiting for input** and the tab enters the waiting band — exactly what a Claude
  `end_turn` does today. `Ready` (`Idle`) is reached only through the recency fallback.
- **Rationale**: chosen by the user as "Claude와 같게". Two agents on one sidebar that mean
  different things by the same word would make the band and the cards unreadable.
  - **Rejected: `Ready` for a finished Antigravity turn.** It would keep such tabs out of the
    band while finished Claude tabs are in it.

### D3: An unknown agent state shows no status
- **Status**: resolved (user, at the mirror and at L2)
- **Decision**: when a tab has an agent and its state is `Unknown`, the card **hides** its
  status zone — no icon, no label — and the branch lays out as it does when the
  `show_agent_status` toggle is off. This covers every agent: Codex, Gemini and Aider always
  (no reader exists), and Claude or Antigravity until their session can be read. A tab with
  no agent keeps the shell statuses (`Running` / `Exited N` / `zsh`) unchanged.
- **Rationale**: the card should state only what is known. This **reverses
  `sidebar-weather-cards` D7's `Unknown → Working`** row, which was chosen when hiding was not
  an option and "Working" was the least-wrong claim; the user chose hiding for both cases.
  - **Rejected: keep `Working` for Unknown.** It is exactly the false claim this spec exists
    to remove (the observed Antigravity card).
  - **Rejected: show the agent's name.** The skin and the icon already name the agent.

### D4: A permission dialog on screen reads Waiting for input — on the card as in the band
- **Status**: resolved (auto, from L1 + the confirmed goal)
- **Decision**: when the on-screen detector (`prompt.rs`) sees a blocking dialog on a tab
  with an agent, that tab's **card** reads **Waiting for input**, whatever its log-derived
  state, for any agent kind. The card and the band read the **same** per-tab result: the
  scan already computed in `push_agent_states` is kept per tab (D7) and handed to the card,
  not re-derived. One function, `effective_state(agent state, dialog level)`, produces the
  state both consumers use — the band's list in `push_agent_states` and the card's status in
  `resolve_status`. Both are recomputed on the same inputs: `push_agent_states` already runs
  on every worker publication, PTY chunk and tab switch (`terminal_app.rs:1107, :1791,
  :2265`), and D7(b)'s return-to-bottom rescan goes through `push_agent_states_for`; the card's rows re-fill on a worker publication (the `MetaSource` generation) and
  on a level change (D7's re-sync). They can differ only between a publication and the next
  frame that pulls it, never in steady state.
- **Rationale**: the log cannot show a pending permission for either agent (D1; Claude's
  `tool_use` reads `Busy`), the screen can, and the scan already runs per PTY chunk for every
  agent tab. Sharing one result is what makes the band and the card unable to disagree (the
  inferred goal).
  - **Rejected: a second detection for the card.** Two scans of one screen can race on a
    chunk boundary and disagree.
  - **Rejected: teaching the worker about dialogs.** The worker runs off the UI thread and has
    no grid; `term-core` must stay policy-free (C4).

### D5: Antigravity's dialog is recognised by its question with the first choice under it
- **Status**: resolved (user); **amended at verification, 2026-09-28 (user-approved)** — see
  the amendment below. The original text follows it, kept for the record.
- **Amendment**: Antigravity's dialog is a **question row whose next non-blank row is its
  first choice** (`1. Yes, …`, optional `>` pointer) — not two of three markers, and no footer
  marker. Claude's two-of-three rule is unchanged, and the two rules never pool.
  - **Why**: the capture D12 took showed `agy` drawing its `esc to cancel … <model>` row under
    **every** running turn (`Generating...` screens: 8 of 58 distinct tails, 24 scans), not
    only under dialogs. With the footer as a marker a busy agent always held one of three, so
    a single ordinary line — the user's own `> 1. Yes, go ahead` echoed back — made a false
    "waiting", the one-line rule this decision had rejected. The Gate-3 checker found it.
  - **Why adjacency**: in the captured dialog the question and the first choice are
    consecutive rows (`Run this command?` / `> 1. Yes, run command`); an echoed user reply sits
    under a separator and a question in the agent's own output is followed by more output, so
    neither can pair with the other. Blank rows between the two are allowed.
  - **Cost, accepted**: the URL, tool and edit dialogs are unmeasured; if one draws something
    other than blank rows between its question and its first choice it is not detected — a
    miss, never a false "waiting".
- **Original decision**: `prompt.rs` gains Antigravity's markers as a **second vocabulary** beside
  Claude's, under the **same** rule applied **per vocabulary** — at least two of one
  vocabulary's three markers in the bottom `TAIL_ROWS` content rows. Markers are never pooled
  across vocabularies (Claude's `❯ 1.` plus Antigravity's `esc to cancel` is not a dialog).
  Antigravity's markers:
  - **question**: a line that is exactly one of `Run this command?`, `Allow access to this
    URL?`, `Allow calling this tool?`, `Do you want to proceed?` (the edit dialog, D6), or that
    ends with `requires permission to read, edit, and execute files here.` (D6);
  - **footer**: a line that starts with `esc to cancel` (lower-case — Antigravity draws the
    model name on the same row);
  - **choice**: a line whose first option reads `1. Yes, …` after an optional pointer glyph.
  Claude's markers are unchanged. The exact bytes of each marker are pinned from a real
  captured screen (D12); Antigravity's vocabulary ships only together with that fixture.
- **Rationale**: chosen by the user. A single line is common enough in ordinary output (an
  agent quoting its own docs) to make a false "waiting" — the failure this project ranks
  worst. Two markers from different parts of the dialog are what the Claude detector already
  relies on (`REQUIRED_MARKERS`).
  - **Rejected: one question line is enough.** Faster, but wrong on any output that quotes it.
  - **Rejected: matching Claude's markers with looser case or spacing.** Claude's footer needs
    `to amend`, which Antigravity writes as `tab Amend` on a different row from its `esc to
    cancel`; loosening Claude's rule would weaken it for Claude too.

### D6: Every Antigravity dialog that blocks on the user counts, workspace trust included
- **Status**: resolved (user)
- **Decision**: the dialogs that read Waiting for input are the ones that stop the agent until
  answered: run a command, access a URL, call a tool, accept an edit (`Do you want to
  proceed?`), and the workspace-trust question at startup (`… requires permission to read,
  edit, and execute files here.`).
- **Rationale**: chosen by the user — each of them stops progress until answered, which is
  what "Waiting for input" means on the card and in the band.
  - **Rejected: tool permissions only.** A tab stuck on the trust question would read as if
    nothing were wrong.

### D7: The card follows a dialog as fast as the band does, background tabs included
- **Status**: resolved (auto)
- **Decision**: the per-tab result of the scan (D4) is stored on the tab's `Session` as a
  **level** — `true` while a dialog is on screen — and updated on these events only:
  - **(a) a scan event** — an advancing PTY chunk of the tab, a worker publication
    (`OverlayPending`), or a tab switch (the three existing `push_agent_states` callers): if
    the tab has an agent and is at the live bottom (`view_offset == 0`), level = the scan; if
    it has an agent but is scrolled back, the level **keeps its last live value** (the
    scrolled view is history, not the live screen); if it has no agent, level = `false`;
  - **(b) the view returning to the live bottom** (`scroll_to_bottom`, a wheel back to 0, a
    snap-to-live): that tab is rescanned once through the existing single-tab
    `push_agent_states_for(tab)` (`terminal_app.rs:1129`), so the band is fed in the same step
    as the level changes;
  - **(c) the agent leaving** (`agent` becomes `None`): level = `false`. A change of that level in either direction
  re-syncs the sidebar rows and requests a redraw, the way a command start or end does
  (`sidebar-weather-cards` D8, `drain_effects`). The scan already runs on every advancing PTY
  chunk of every agent tab, visible or not, so answering a dialog clears the card on the next
  chunk the agent draws.
- **Rationale**: a level cannot get stuck the way a latched event can; answering removes the
  dialog, the next scan reads `false`, and the card falls back to the log state.
  - **Rejected: re-syncing every frame.** It would reintroduce per-frame work at idle (G1).

### D8: The card's status precedence, with the dialog first
- **Status**: resolved (auto; extends `sidebar-weather-cards` D7)
- **Decision**: `resolve_status` becomes, first match wins:
  1. an agent is present and its tab shows a dialog (D4) → `WaitingForInput`;
  2. an agent is present → its state: `Busy` → `Working`, `Idle` → `Ready`, `Waiting` →
     `WaitingForInput`, **`Unknown` → no status** (D3);
  3. no agent: running → `Running`; a reportable last exit → `Exited N`; else `Shell` — unchanged.
- **Rationale**: the screen is the only source that can see a pending permission (D4), so it
  outranks the log; everything below it is `sidebar-weather-cards` D7 with D3's one change.

### D9: The renderer gains a "no status" value, drawn like the toggle-off card
- **Status**: resolved (auto)
- **Decision**: `renderer::sidebar_card::CardStatus` gains a variant for "no status". For it,
  `build_card` emits no status icon and no status run, and the branch lays out exactly as it
  does with `show_agent_status` off (right-aligned, its floor at the padding box's left). The
  renderer stays agent-agnostic: it is told "no status", not "unknown agent".
- **Rationale**: the global toggle already defines what a card without a status looks like
  (`sidebar-weather-cards` D13); reusing that layout keeps one definition of it.
  - **Rejected: `Option<CardStatus>` on `RowMeta`.** Every consumer would re-derive the same
    "none" branch; one variant keeps the precedence in `resolve_status`.

### D10: ai-monitor classifies an Antigravity step into Claude's `LastMessage`; app keeps the policy
- **Status**: resolved (auto, from L1 layering)
- **Decision**: `crates/ai-monitor` gains a tail reader for an Antigravity transcript (8 KiB,
  the same `TAIL_CAP`) that returns the **existing** `activity::LastMessage` of its last
  meaningful step — reply → `AssistantEndTurn` (or `None`, no robust signal, after background
  work: D13/D14), `ask_question` dispatch → `AssistantAsksUser`, other dispatch →
  `AssistantBusy`, tool result → `UserToolResult`, `USER_INPUT` → `UserPrompt` (D1's table) —
  plus the file's `mtime`/`len`. The app's `state_from` maps it to `AgentState`
  **unchanged**.
- **Rationale**: the split `specs/agent-asks-user-waiting` set — ai-monitor supplies the raw
  classification, app owns the state policy — and reusing `LastMessage` makes "the same state
  means the same thing for both agents" (D2) structural rather than a promise: one
  `state_from`, one recency fallback, one conservative bias.
  - **Rejected: a separate `AgentState` rule for Antigravity in app.** Two rules drift.

### D11: The worker resolves a tab's Antigravity transcript, memoised and bounded
- **Status**: resolved (auto)
- **Decision**: `worker.rs`'s `activity` handles `AgentKind::Antigravity` alongside Claude: it
  asks `presence_fd::conversation_of_pid(pid)` for the conversation the tab's `agy` holds,
  resolves it with `ai_monitor::antigravity::resolve`, and reads the tail as D10. The
  resolution is memoised per pid and re-probed **at most once per `RESOLVE_TTL_TICKS`** (the
  Claude path's existing TTL, ≈10 s) — resolved or not, so an `agy` that is `LiveButUnnamed`
  costs one FD listing per TTL, not one per tick; a new pid is probed at once; the tail read is memoised on
  `(mtime, len)` as Claude's is (`tail_role`, R6.7). An unresolvable session is `Unknown` →
  no status (D3).
- **Rationale**: the FD listing is a syscall per open file, so it must not run every tick for
  every tab; the memo + TTL bounds it to one probe per Antigravity tab per TTL while a session
  is stable. `presence_fd` already belongs to the app crate, which the worker is part of, so
  ai-monitor stays headless (AG11).
  - **Rejected: asking the ai-monitor panel.** The panel resolves only the tab it shows; the
    sidebar needs every tab.
  - **Assumption A1 (named, not enforced):** an `agy` process holds one conversation for at
    least one TTL. If it starts or resumes another conversation inside the same process, the
    card can show the previous conversation's state — including a finished turn's
    `Waiting` — for up to one TTL, until the re-probe finds the new lock.

### D12: Antigravity's markers are pinned from a real captured screen
- **Status**: resolved (auto; `mirror-from-the-rendered-design-file` / "measure, don't infer")
- **Decision**: before D5's markers are relied on, a real Antigravity permission dialog is
  captured from this terminal: the app run with `RUST_LOG=app=debug` logs the scanned tail on
  every agent-tab scan (`waiting-scan`, `terminal_app.rs`), and that text becomes the
  detector's Antigravity fixture, as `REAL_DIALOG` is Claude's. **Correction found at L4:**
  that line carries only the last **six** rows today (`tab_scan_tail`, `take(6)`), fewer than
  an Antigravity dialog spans (question, choices, a two-row footer); the dump is widened to the
  full `TAIL_ROWS` the matcher reads, Debug-escaped so the fixture is byte-exact (T0), before
  the capture is taken. The pointer glyph, the footer
  row and the choice wording are taken from it, not from a screenshot or the binary's strings.
  The capture is **pre-work owned by the user** (it needs a real dialog on screen), and it
  **blocks** the task that adds Antigravity's vocabulary: no fixture, no markers. The
  workspace-trust question (D6) is gated the same way on **its own** capture (running `agy`
  in a folder it has not seen); without that capture the trust marker does not ship, and D6's
  trust case stays a Known Gap.
- **Rationale**: the screenshot cannot show which glyph the pointer is, and the binary's
  strings are fragments. Claude's detector was built the same way and was right the first
  time it met a real dialog; the ones built from reasoning were not (`specs/agent-prompt-
  waiting`'s "two wrong fixes").

### D13: A reply after background work is not a finished turn
- **Status**: resolved (user, from the inversion probe)
- **Amended by `specs/agy-background-working` D1**: a reply whose turn started a background
  task or subagent that has not yet reported back (no completion `SYSTEM_MESSAGE`) reads
  `AssistantBusy` → `Working`, at any age. Everything below still holds when nothing is pending.
- **Decision**: a tool-less `PLANNER_RESPONSE` is treated as a finished turn (`Waiting`, D2)
  only when **every** step since the last `USER_INPUT` is known foreground work: no step has
  `status: RUNNING`, there is no `INVOKE_SUBAGENT` step, and every dispatched tool is in the
  closed **allow-list** `run_command`, `view_file`, `list_dir`, `grep_search`, `find_by_name`,
  `write_to_file`, `replace_file_content`, `ask_question`. Anything else — a background tool
  (`invoke_subagent`, `define_subagent`, `manage_subagents`, `send_message`, `manage_task`,
  `schedule`) **or any tool name not on the list** — makes the reply **no robust signal**, and
  the recency fallback decides: `Working` within 10 s of the transcript's last write (`mtime`,
  as `state_from` measures it), then `Ready` — never `Waiting`.
- **Rationale**: measured on 111 replies in 78 real transcripts. 21 replies were followed by
  more model steps with no user turn in between — the agent narrating while its subagents or
  background tasks ran ("waiting for the final subagent", 96 s) — and **all 21** had
  background work since the last user input; no reply without background work was ever
  followed by more model steps (0 of 56). Treating every reply as a finished turn would read
  19% of replies as a false "waiting", the failure this project ranks worst.
  - **Cost, accepted by the user:** 34 genuine turn ends that followed background work read
    `Ready` (after 10 s) instead of `Waiting`, and do not enter the band.
  - **Rejected: always `Waiting`** — 21/111 false waits. **Rejected: `Working` until the next
    step** — those 34 turn ends would read `Working` until the user typed again.
  - **Allow-list, not deny-list (L2 review).** Both give the same result on the measured
    corpus (56 `Waiting`, 55 recency, 0 false waits). They differ on a tool this spec has not
    seen: a deny-list would let a reply after a new or renamed background tool read `Waiting`
    — the worst failure — while the allow-list reads it as recency. The measurement covered
    one machine and one `agy` version; the allow-list is what makes that sample safe to
    generalise from.

### D14: The background check reads back to the last user input, bounded like Claude's prompt scan
- **Status**: resolved (auto, from measurement)
- **Decision**: the 8 KiB tail classifies the last meaningful step (D10). **Only when that step
  is a tool-less reply**, and only on a **changed** transcript (new `(mtime, len)`), the reader
  also scans back from the end up to `PROMPT_SCAN_CAP` (1 MiB,
  `crates/ai-monitor/src/activity.rs:102`) to the last `USER_INPUT`, checking the steps on the
  way against D13; the result is memoised with the
  classification, so an unchanged transcript costs no read (R6.7's rule). If no `USER_INPUT`
  lies within the window, background work cannot be ruled out and a reply is **no robust
  signal** (recency, never `Waiting`).
- **Rationale**: an 8 KiB tail reaches the last user input for only 23 of 111 replies
  (median 33 KiB between them, p99 181 KiB, max 203 KiB), while a reply line itself is small
  (median 1.3 KiB, max 6.8 KiB, none over 8 KiB), so a tail-only check would turn
  most finished turns into `Ready` and hollow out D2. The 1 MiB window covers every measured
  case and is the bound the Claude path already pays for its label scan, on the same trigger.
  - **Rejected: an incremental reader** that parses only appended bytes and carries the
    background flag forward. Cheaper per change, but a second stateful parser to keep right;
    the bounded re-read matches existing precedent and its cost is already accepted.

## Constraints
- **Conservative bias.** `Waiting` comes only from a precise signal: a finished turn with no
  background work (D2/D13), a human-blocking dispatch (`ask_question`, Claude's
  `AskUserQuestion`/`ExitPlanMode`), or a dialog on screen (D4). No fallback yields `Waiting`.
  Two named, bounded exceptions: **A1** (D11) — a stale finished turn of the previous
  conversation for at most one TTL after an in-process conversation switch; and **the
  scroll-back hold** (D7a) — a tab whose dialog closed while it was scrolled back keeps
  `Waiting` until its view returns to the live bottom (D7b).
- **Claude is unchanged except for the card and one band rule.** Claude's `state_from`,
  `LastMessage` classification and `prompt.rs` markers stay as they are; what changes for a
  Claude tab is that its card now shows a detected dialog (D4) and hides an `Unknown` state
  (D3), and — for every agent — a scrolled-back tab's band row keeps its last live dialog
  level instead of dropping to the log state (D7a).
  Both dialog rules run on every agent tab (D4: "for any agent kind"), so a Claude tab whose own
  output shows an exact Antigravity question row with a `1. Yes, …` row directly under it reads
  Waiting until that text scrolls out of the tail — the one way Antigravity's rule can reach a
  Claude card (found at verification).
- **Bounded idle work (G1).** No new wake of the UI thread: the dialog scan stays on the
  existing `push_agent_states` events plus D7(b)'s one rescan on returning to the bottom. On
  the worker thread the FD probe runs at most once per Antigravity tab per TTL (D11), and a
  re-probe that finds the same conversation publishes nothing, so it wakes nothing
  (`publish_if_changed`); a transcript is read only when its `(mtime, len)` changed (D14).
- **Headless crates stay headless.** `term-core` gains nothing (C4); `ai-monitor` gains no
  `libc`/FD code — the presence-lock probe stays in `app` (AG11).
- **Read-only on Antigravity's files.** Nothing under `~/.gemini/antigravity-cli` is created,
  locked or written (`specs/ai-monitor-antigravity` Constraints).
- **The renderer stays agent-agnostic.** It learns only a "no status" value (D9).
- **Non-goals hold:** the card's look, the ai-monitor panel, agent detection.
- macOS only; no new dependency; `Cargo.lock` untouched.

## Known Gaps
- **Only the `run_command` dialog is measured.** The capture D12 asked for was taken (T0,
  `captures/`), and it is what showed the footer row to be useless as a marker (D5, amended).
  The URL, tool and edit dialogs are still known only from the binary's strings; a different
  layout there reads as "not detected", never as a false "waiting".
- **The workspace-trust screen (D6) is unmeasured.** Its question is not in the vocabulary
  until a real capture shows it (D12); until then it is not detected.
- **An external format we cannot pin** (same kind as `docs/architecture.md` G6 / A1–A2):
  Antigravity's step names (`PLANNER_RESPONSE`, `USER_INPUT`, …), its `ask_question` and
  background tool names, its append-on-completion behaviour, and its dialog wording. A rename
  degrades quietly — a renamed step or an unlisted tool to the recency fallback (D13's
  allow-list), a wording change to "not detected" — rather than to a false `Waiting`. A renamed
  `ask_question` would lose its `Waiting` the same way.
- **Accepted cost of D13, both halves:** a genuine turn end that followed background work reads
  `Ready` (34 of 90 measured) and does not enter the band; and a reply written *while*
  background work still runs reads `Ready` from 10 s after it until the next step (21
  measured, up to 96 s) although the agent is still working.
  *(Second half amended by `specs/agy-background-working` D1: while that work has not reported
  back, the reply reads `Working`.)*
- **Codex, Gemini and Aider have no state** (D3 hides it). Their dialogs are detected only if
  they happen to match Claude's rule (two of its markers) or Antigravity's (a question row with
  the first choice under it).
- **A dialog outside the live bottom is not seen** (existing `prompt.rs` rule: a scrolled-back
  tab is not scanned). While a tab is scrolled back its level keeps its last live value
  (D7a), so a dialog that appears or closes during scroll-back is reflected only when the view
  returns to the bottom (D7b).

## Requirements

### R0: An agent tab's card states only what is true (Confirmed Goal)

#### R0.1: An Antigravity permission dialog reads Waiting for input on the card and in the band
- **Given**: an Antigravity tab at the live bottom whose screen holds the captured permission
  dialog (D12's fixture)
- **When**: the tab's PTY chunk that draws the dialog is processed
- **Then**: the tab's card resolves to `WaitingForInput` and the waiting band lists the tab

#### R0.2: Answering the dialog returns the card to Working
- **Given**: the tab of R0.1, card `WaitingForInput`, transcript's last meaningful step a
  `run_command` dispatch
- **When**: the dialog is answered and the next PTY chunk no longer shows it
- **Then**: the card resolves to `Working` and the tab leaves the band

#### R0.3: A finished Antigravity turn reads Waiting for input
- **Given**: an Antigravity tab whose transcript ends in a tool-less `PLANNER_RESPONSE` with
  only foreground work since the last `USER_INPUT`
- **When**: the worker publishes the tab
- **Then**: the card resolves to `WaitingForInput` and the tab is in the band, as a Claude
  `end_turn` is

#### R0.4: A Claude permission dialog reads Waiting for input on the card
- **Given**: a Claude tab at the live bottom showing the real Claude permission dialog
  (`prompt.rs`'s `REAL_DIALOG`) while its log reads `Busy`
- **When**: the tab's PTY chunk is processed
- **Then**: the card resolves to `WaitingForInput` (today it reads `Working`)

#### R0.5: An agent with no readable state shows no status
- **Given**: a tab running Codex, Gemini or Aider (agent state `Unknown`)
- **When**: its card is built
- **Then**: no status icon and no status label are drawn; the branch still is

#### R0.6: The workspace gates are green
- **Given**: the finished change
- **When**: `cargo build`, `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` run, and `cargo mutants` runs on
  every changed file
- **Then**: all three exit 0, and every surviving mutant is argued equivalent in writing

### R1: ai-monitor classifies an Antigravity transcript's last meaningful step (D1, D10)

#### R1.1: A tool-less reply after foreground work is a finished turn
- **Given**: a transcript whose last meaningful step is a `PLANNER_RESPONSE` without
  `tool_calls`, with only allow-listed dispatches and no `RUNNING`/`INVOKE_SUBAGENT` step since
  the last `USER_INPUT`
- **When**: the Antigravity tail reader reads it
- **Then**: it returns `LastMessage::AssistantEndTurn`

#### R1.2: An `ask_question` dispatch is a human-blocking call
- **Given**: a transcript whose last meaningful step is a `PLANNER_RESPONSE` whose
  `tool_calls` include `ask_question`
- **When**: the reader reads it
- **Then**: it returns `LastMessage::AssistantAsksUser`

#### R1.3: Any other dispatch is busy
- **Given**: a last meaningful step that is a `PLANNER_RESPONSE` whose `tool_calls` name only
  tools other than `ask_question` (e.g. `run_command`)
- **When**: the reader reads it
- **Then**: it returns `LastMessage::AssistantBusy`

#### R1.4: Every step kind of the closed result set is a tool result
- **Given**: a last meaningful step of each kind `GENERIC`, `RUN_COMMAND`, `VIEW_FILE`,
  `LIST_DIRECTORY`, `GREP_SEARCH`, `CODE_ACTION`, `FIND`, `INVOKE_SUBAGENT`, `ASK_QUESTION`, with
  `status` `DONE` and with `status` `RUNNING`
- **When**: the reader reads it
- **Then**: it returns `LastMessage::UserToolResult` for each

#### R1.5: A user input is busy
- **Given**: a last meaningful step of kind `USER_INPUT`
- **When**: the reader reads it
- **Then**: it returns `LastMessage::UserPrompt`

#### R1.6: Bookkeeping steps are skipped
- **Given**: a transcript ending in a reply (R1.1's shape) followed by `CHECKPOINT`,
  `SYSTEM_MESSAGE`, `CONVERSATION_HISTORY` and `ERROR_MESSAGE` steps
- **When**: the reader reads it
- **Then**: it returns the reply's classification, `AssistantEndTurn`

#### R1.7: An unlisted step kind is no robust signal
- **Given**: a last step whose kind is none of R1.1–R1.6's (e.g. `SOME_NEW_KIND`)
- **When**: the reader reads it
- **Then**: it returns `last: None`

#### R1.8: An unparseable tail is no robust signal, not an error
- **Given**: a transcript whose last line is longer than `TAIL_CAP` (8 KiB), or an empty file
- **When**: the reader reads it
- **Then**: it returns a state with `last: None` and the file's `mtime`/`len` (never `None`
  for an existing readable file)

#### R1.9: The reader reports what the recency fallback needs
- **Given**: any readable transcript
- **When**: the reader reads it
- **Then**: it returns the file's `mtime` and `len`, and a `role` that is never `"user"` when
  `last` is `None` — so the fallback is exactly D1's (`Busy` within 10 s, else `Idle`)

### R2: A reply after background work is not a finished turn (D13, D14)

#### R2.1: A background task since the last user input
- **Given**: a tool-less reply as the last meaningful step, with a `status: RUNNING` step
  between it and the last `USER_INPUT`
- **When**: the reader reads it
- **Then**: it returns `last: None`

#### R2.2: A subagent since the last user input
- **Given**: the same, with an `INVOKE_SUBAGENT` step in between
- **When**: the reader reads it
- **Then**: it returns `last: None`

#### R2.3: A dispatch outside the allow-list, named or unseen
- **Given**: the same, with a dispatch of `manage_task` in one case and of a tool name no
  transcript has used (`frobnicate`) in another
- **When**: the reader reads each
- **Then**: both return `last: None`

#### R2.4: The allow-list reads as foreground
- **Given**: the same, with one dispatch of each of `run_command`, `view_file`, `list_dir`,
  `grep_search`, `find_by_name`, `write_to_file`, `replace_file_content`, `ask_question` in
  between and nothing else
- **When**: the reader reads it
- **Then**: it returns `AssistantEndTurn`

#### R2.5: The back-scan reaches a user input beyond the tail
- **Given**: a tool-less reply whose last `USER_INPUT` lies more than `TAIL_CAP` and less than
  `PROMPT_SCAN_CAP` (1 MiB) before the end, with only foreground work between
- **When**: the reader reads it
- **Then**: it returns `AssistantEndTurn`; with a `RUNNING` step inserted in that span it
  returns `None`

#### R2.6: No user input within the window is no robust signal
- **Given**: a tool-less reply with no `USER_INPUT` in the last `PROMPT_SCAN_CAP` bytes
- **When**: the reader reads it
- **Then**: it returns `last: None`

#### R2.7: The back-scan runs only after a tool-less reply
- **Given**: a transcript larger than `TAIL_CAP` whose last meaningful step is a dispatch, a
  tool result or a `USER_INPUT`
- **When**: the reader reads it
- **Then**: it reads no more than `TAIL_CAP` bytes (observed through the byte count the
  reader exposes to tests)

### R3: The app maps the classification with Claude's unchanged rule (D2, D10)

#### R3.1: The same `state_from` serves both agents
- **Given**: an Antigravity tail state with `last` = each of `AssistantEndTurn`,
  `AssistantAsksUser`, `AssistantBusy`, `UserToolResult`, `UserPrompt`
- **When**: the worker derives the tab's `AgentState`
- **Then**: it is `Waiting`, `Waiting`, `Busy`, `Busy`, `Busy` — through the existing
  `state_from`, whose body and tests are unchanged

#### R3.2: The fallback never yields Waiting for Antigravity
- **Given**: an Antigravity tail state with `last: None`, `mtime` 5 s, 50 s and 200 s old
- **When**: the worker derives the state
- **Then**: it is `Busy`, `Idle`, `Idle` — never `Waiting`, never `Unknown`

### R4: The worker resolves each Antigravity tab's transcript, bounded (D11)

#### R4.1: A tab's lock names the transcript it reads
- **Given**: an `agy` tab whose pid holds conversation C's presence lock, C's transcript on
  disk, and a different conversation D more recent in the same cwd's history
- **When**: the worker refreshes the tab
- **Then**: the published state is derived from C's transcript, not D's

#### R4.2: An unresolvable session is Unknown
- **Given**: an `agy` tab whose pid holds no readable lock (`LiveButUnnamed`), or whose
  conversation has no transcript yet
- **When**: the worker refreshes the tab
- **Then**: the published agent state is `Unknown` (→ no status, R5.2)

#### R4.3: The probe runs at most once per TTL, resolved or not
- **Given**: one `agy` tab, refreshed every tick for `3 × RESOLVE_TTL_TICKS` ticks with the
  pid unchanged — once with a resolving lock, once with none
- **When**: the probe calls are counted (the probe is injected, as `start_ms_of` is)
- **Then**: each case makes at most 3 probe calls

#### R4.4: A new pid is probed at once
- **Given**: a tab whose memoised probe is younger than the TTL
- **When**: its `agy` pid changes
- **Then**: the next refresh probes the new pid on that tick

#### R4.5: An unchanged transcript is not re-read
- **Given**: a resolved transcript whose `(mtime, len)` is unchanged since the last read
- **When**: the tab is refreshed again
- **Then**: the tail reader is not called; the memoised state is reused

#### R4.6: The published agent pid stays Claude-only
- **Given**: an `agy` tab that the worker probes
- **When**: the worker publishes it
- **Then**: its `TabMeta::agent_pid` is `None` — the existing pin
  `only_a_claude_kind_agent_is_published` passes unmodified, so the panel's pid compare cannot
  turn into a per-tick re-point storm

#### R4.7: Agents without a reader are not probed
- **Given**: a Codex, Gemini or Aider tab
- **When**: the worker refreshes it
- **Then**: no probe runs, no transcript is read, and the state is `Unknown`

#### R4.8: The agent-status toggle off stops the probe
- **Given**: an `agy` tab with `show_agent_status` off
- **When**: the worker refreshes it
- **Then**: no probe runs and no transcript is read, as the existing toggle already stops
  Claude's log reads

### R5: The card's status precedence and the "no status" value (D3, D8, D9)

#### R5.1: A dialog outranks every agent state
- **Given**: an agent present, each `AgentState`, and the tab's dialog level `true`
- **When**: `resolve_status` runs
- **Then**: it returns `WaitingForInput` for all four

#### R5.2: Without a dialog, the agent's state decides, Unknown hides
- **Given**: an agent present, dialog level `false`
- **When**: `resolve_status` runs for `Busy`, `Idle`, `Waiting`, `Unknown`
- **Then**: it returns `Working`, `Ready`, `WaitingForInput`, and the no-status variant

#### R5.3: A tab with no agent keeps the shell statuses
- **Given**: no agent, dialog level `true` or `false`
- **When**: `resolve_status` runs with running / reportable exit / neither
- **Then**: it returns `Running`, `Exited N`, `Shell` — unchanged, the level ignored

#### R5.4: The no-status card lays out as the toggle-off card
- **Given**: one card input, built once with the no-status variant and `show_status` on, and
  once with any status and `show_status` off
- **When**: `build_card` runs for both, at 1× and 2×
- **Then**: both emit no status icon and no status run, and their shapes and text runs are
  identical

#### R5.5: A no-status card carries no status ink in the captured frame
- **Given**: a sidebar frame with one `Working` card and one no-status card (golden-frame
  harness, 1× and 2×)
- **When**: the frame is captured through `Renderer::capture`
- **Then**: the `Working` card's status-icon box holds the status icon's colour and the
  no-status card's same box holds only the card background

### R6: `prompt.rs` recognises Antigravity's dialogs (D5, D6, D12)

#### R6.1: The captured Antigravity dialog is detected
- **Given**: the real Antigravity permission dialog captured with `RUST_LOG=app=debug`
  (`waiting-scan`) and checked in as the detector's Antigravity fixture
- **When**: `shows_blocking_prompt` reads it
- **Then**: it returns `true`

#### R6.2: A busy turn, or half a dialog, is not a dialog (as amended with D5)
- **Given**: the captured busy screen (`captures/antigravity_generating.txt`, which carries
  `esc to cancel`) alone and with one added line each — the user's `> 1. Yes, go ahead` echo,
  a `Do you want to proceed?` line in the output, and both a separator apart; and the dialog
  fixture with its question row, or its choice rows, removed
- **When**: `shows_blocking_prompt` reads each
- **Then**: all return `false`

#### R6.3: The two agents' rules do not pool
- **Given**: an Antigravity question over Claude's chooser (`❯ 1. Yes`), a Claude question over
  Antigravity's first choice, and Antigravity's first choice over Claude's footer
- **When**: `shows_blocking_prompt` reads it
- **Then**: it returns `false`

#### R6.4: Every blocking question counts, as an exact line
- **Given**: a first choice row directly under a question row of, in turn, `Run this
  command?`, `Allow access to this URL?`, `Allow calling this tool?`, `Do you want to
  proceed?` — and, as negatives, `Run this command? (y/n)` and `see "Run this command?" in the
  docs`
- **When**: `shows_blocking_prompt` reads each
- **Then**: the four questions return `true`; both negatives return `false`

#### R6.5: The workspace-trust question counts once it is captured
- **Given**: a real capture of `agy`'s workspace-trust screen, checked in as a second fixture
- **When**: `shows_blocking_prompt` reads it
- **Then**: it returns `true`. Without that capture the trust suffix is not in the vocabulary
  (D12) and D6's trust case stays a Known Gap

#### R6.6: Claude's detection is unchanged
- **Given**: every pre-existing `prompt.rs` test (Claude's `REAL_DIALOG` and its negatives)
- **When**: they run
- **Then**: they pass with their inputs and expectations unmodified

#### R6.7: A dialog above the scanned tail is not detected
- **Given**: the Antigravity fixture followed by more than `TAIL_ROWS` rows of output
- **When**: `shows_blocking_prompt` reads the screen
- **Then**: it returns `false`

### R7: Each tab keeps a dialog level, updated only on named events (D7)

#### R7.1: A live scan sets and clears the level
- **Given**: an agent tab at the live bottom
- **When**: a PTY chunk draws a dialog, and a later chunk removes it
- **Then**: the tab's level is `true` after the first and `false` after the second

#### R7.2: A scrolled-back tab keeps its last live level
- **Given**: an agent tab whose level is `true`, then scrolled back (`view_offset > 0`)
- **When**: a PTY chunk, a worker publication and a tab switch each trigger a scan
- **Then**: the level stays `true` and the band row stays; the screen is not scanned

#### R7.3: A tab without an agent has no level
- **Given**: a tab with no agent whose screen shows a Claude or Antigravity dialog
- **When**: any scan event fires
- **Then**: its level is `false` and it is not in the band

#### R7.4: Returning to the live bottom rescans once and feeds the band
- **Given**: the scrolled-back tab of R7.2 whose dialog closed while scrolled back
- **When**: its view returns to the bottom by a wheel reaching offset 0, and separately by
  `snap_tab_to_live`
- **Then**: in that same step the tab is rescanned once via `push_agent_states_for`, its level
  becomes `false`, its card leaves `WaitingForInput` and it leaves the band

#### R7.5: The agent leaving clears the level
- **Given**: an agent tab with level `true`
- **When**: a worker publication reports `agent: None` for it
- **Then**: its level is `false`

#### R7.6: A level change re-syncs the rows; no change does nothing
- **Given**: an agent tab
- **When**: a scan flips its level (either direction), and when a scan leaves it unchanged
- **Then**: the first re-syncs the sidebar rows and requests a redraw, as a command start or
  end does (`drain_effects`); the second does neither

#### R7.7: A background tab's card follows its own dialog
- **Given**: an agent tab that is not the active tab
- **When**: its PTY chunk draws a dialog
- **Then**: its level becomes `true` and its card reads `WaitingForInput` without a tab switch

### R8: One effective state feeds the band and the card (D4)

#### R8.1: `effective_state` is the dialog override
- **Given**: every `AgentState` and each dialog level
- **When**: `effective_state(state, level)` runs
- **Then**: it returns `Waiting` when the level is `true`, else the state unchanged

#### R8.2: The band reads the effective state
- **Given**: an agent tab with level `true` and a log state `Busy`
- **When**: `push_agent_states` feeds the waiting model
- **Then**: the tab is handed over as `Waiting` and is in the band

#### R8.3: The card reads the same level
- **Given**: the tab of R8.2
- **When**: the sidebar rows are filled
- **Then**: its card resolves to `WaitingForInput`, from the same stored level the band used —
  no second scan of the screen

### R9: The change's bounds hold (Constraints)

#### R9.1: An idle Antigravity tab costs one probe per TTL and wakes nothing
- **Given**: an `agy` tab whose lock and transcript do not change, refreshed every tick for
  `3 × RESOLVE_TTL_TICKS` ticks after its first publication
- **When**: probes, transcript reads and publications are counted
- **Then**: at most 3 probes, 0 transcript reads, 0 publications

#### R9.2: Antigravity's files are only read
- **Given**: a fake Antigravity root (brain, presence, history) in a temp dir
- **When**: the worker resolves and reads a tab against it
- **Then**: the tree's listing and every file's bytes and `mtime` are identical before and
  after

#### R9.3: The headless crates gain nothing forbidden
- **Given**: the finished change
- **When**: `cargo tree -p term-core` and `cargo tree -p ai-monitor` are inspected and
  `ai-monitor`'s source is searched for `libc`/`proc_pid`
- **Then**: neither crate gains a dependency, and `ai-monitor` has no FD code; `Cargo.lock` is
  unchanged

## Tasks

### T0: Widen the `waiting-scan` debug dump to the matcher's full input [infra]
- **Fulfills**: R6.1 (its capture precondition, D12), R0.6
- **Depends on**: (none) ← parallel with T1, T2
- **Files**: `crates/app/src/terminal_app.rs` (`tab_scan_tail`)
- **Notes**: log all `TAIL_ROWS` rows exactly as `viewport_tail_text` returns them,
  Debug-escaped (`{:?}`), still behind `tracing::enabled!(DEBUG)` so release with logging off
  pays nothing. Unblocks the Pre-work capture, which can then run while T1–T4 proceed.

### T1: The "no status" card [vertical — renderer]
- **Fulfills**: R5.4, R5.5, R0.6
- **Depends on**: (none) ← parallel with T0, T2
- **Files**: `crates/renderer/src/sidebar_card.rs`, `crates/renderer/tests/golden_frame.rs`
- **Notes**: a `CardStatus` variant (D9) that `build_card` lays out through the same path as
  `show_status` off; the golden frame asserts the pixel half (CLAUDE.md: a render change is
  not covered until something asserts what reached the screen).

### T2: ai-monitor's Antigravity tail reader [internal service — ai-monitor]
- **Fulfills**: R1, R2, R9.3 (ai-monitor half), R0.6
- **Depends on**: (none) ← parallel with T0, T1
- **Files**: `crates/ai-monitor/src/antigravity.rs` (or a sibling module it re-exports),
  `crates/ai-monitor/tests/antigravity.rs` and fixtures
- **Notes**: returns the existing `activity::TailState` (`last`, `role`, `mtime`, `len`); the
  8 KiB tail classifies, the ≤ 1 MiB back-scan runs only after a tool-less reply (D14). No FD
  or `libc` code (AG11).

### T3: The worker derives Antigravity's state [vertical — worker → published state → existing card and band]
- **Fulfills**: R3, R4, R9.1, R9.2, R9.3 (app half), R0.3, R0.6
- **Depends on**: T2 ← parallel with T1, T4
- **Files**: `crates/app/src/tabmeta/worker.rs`, `crates/app/src/tabmeta/activity.rs`
- **Notes**: probe injected on `Deriver` as `agent_of`/`start_ms_of` are (R4.3's counter); the
  Antigravity root injected so R9.2 runs against a temp tree; published `agent_pid` stays
  Claude-only (R4.6). `Waiting` already maps to `WaitingForInput` and the band, so R0.3 is
  whole here.

### T4: One dialog level per tab feeds the band and the card [vertical — scan → Session → band + card]
- **Fulfills**: R5.1, R5.2, R5.3, R7, R8, R0.4, R0.5, R0.6
- **Depends on**: T0 (both edit `terminal_app.rs`), T1 (the variant) ← parallel with T2, T3
- **Files**: `crates/app/src/session.rs`, `crates/app/src/terminal_app.rs`,
  `crates/app/src/tabs.rs`, `crates/app/src/sidebar_model.rs`,
  `crates/renderer/src/sidebar_row.rs` (the row carries the level to `row_meta`),
  `specs/sidebar-weather-cards/spec.md` (a pointer at D7: `Unknown → Working` superseded by
  this spec's D3)
- **Notes**: `effective_state` is the single override both consumers call; D7's three events
  and the `push_agent_states_for` rescan on return to bottom; `resolve_status` gains the
  level and D8's order.

### T5: Antigravity's dialog vocabulary, end to end [vertical — fixture → detector → card and band]
- **Fulfills**: R6, R0.1, R0.2, R0.6
- **Depends on**: T3, T4, and the Pre-work capture
- **Files**: `crates/app/src/prompt.rs` and its Antigravity fixture(s);
  `docs/architecture.md` (Antigravity's step names, tool names and dialog wording recorded
  beside G6 as an external format this repo cannot pin — the spec's Known Gaps)
- **Notes**: a second rule beside Claude's, never pooled — as amended at verification, a
  question row with the first choice as its next non-blank row (D5); Claude's tests untouched
  (R6.6). The trust marker ships only with its own capture (R6.5).

## External Dependencies

### Pre-work
- **Capture a real Antigravity permission dialog (user, blocks T5; needs T0).** Run the app
  with `RUST_LOG=app=debug`, have `agy` ask for a tool permission (e.g. a `run_command`), and
  keep the `waiting-scan` line logged while the dialog is up. That line becomes the fixture.
- **Optional: capture the workspace-trust screen (user, gates R6.5).** Run `agy` in a folder it
  has not seen, with the same logging. Without it the trust marker does not ship and D6's
  trust case stays a Known Gap.

### Post-work
- `/run-term` eyeball: an Antigravity dialog → card "Waiting for input" and a band row;
  answering → "Working"; a finished Antigravity turn → "Waiting for input"; a Claude
  permission dialog → card "Waiting for input"; a Codex tab → no status, branch shown.
- Commit only when the user asks.

## Implementation Notes
Where the code settled a detail the decisions left open, or departs from their wording:

- **D11 — tier 1 only, without `resolve_in`.** The worker builds the transcript path with
  `ai_monitor::antigravity::transcript_in` from the probed id instead of calling
  `resolve_in(Probed::Conversation(id), cwd)`. The outcome is identical (`LiveButUnnamed` →
  no transcript; a named conversation → its transcript if the file exists), but `resolve_in`
  also reads the transcript tail for the panel's `last_event_ms`, which the card does not use
  and would pay on every probe (R9.1). Module: `crates/app/src/tabmeta/antigravity.rs`.
- **D13 — "known foreground work" includes the step kind.** A step between the last
  `USER_INPUT` and the reply whose `type` is not one the reader knows (a result kind,
  bookkeeping, `PLANNER_RESPONSE`), or has no `type`, is not foreground: the reply is no
  robust signal. The allow-list's reasoning applied to step kinds.
- **Research, re-counted at T2:** 1,336 `PLANNER_RESPONSE` steps with `tool_calls` (1,447 minus
  111 replies) against the same 3,045 steps, where L1 wrote 1,333; 79 transcripts rather than
  78 (the corpus is live). No outcome of D13's measurement moved.
- **D13/D1 — an empty `tool_calls: []`** counts as a reply (it dispatched nothing), as
  telemetry's turn count already treats it. Unmeasured: every `tool_calls` in the corpus is
  non-empty.
- **D7 — the rule lives on `Session`, the scan on `Tabs`.** `Session::rescan_dialog(has_agent)`
  applies D7(a)/(c) (`next_dialog_level`) and stores the level; `Tabs::rescan_dialogs` runs it
  for every tab `push_agent_states` feeds and returns the band's states plus the flips;
  `rescan_at_live_bottom` (wheel reaching offset 0, `snap_tab_to_live`) is D7(b). A flip
  patches that one row in place (`TabModelHandle::set_blocking_dialog`) rather than calling
  `sync_tab_model`: the scan also runs on a metadata publication, and `Tabs::titles` re-sends
  the roster to the worker, which would reopen the publish → derive → publish path
  card-live-cwd R5.2 closed (found by the Gate-3 checker). The remaining joins inside
  `Running`, which no test can call, are pinned by a source scan
  (`the_dialog_level_is_wired_where_no_test_can_call`), the precedent `seam.rs` set.
- **D5 — amended** (see D5): question row + next non-blank row is the first choice. The choice
  is `1. Yes, ` after an optional ASCII `>`: the captured pointer is `>`, and it moves with the
  selection, so option 1 can be drawn without it. A second capture,
  `captures/antigravity_generating.txt`, is the negative fixture.
- **D6/D12 — the trust marker did not ship.** No capture of the workspace-trust screen was
  taken; `the_trust_question_waits_for_its_own_capture` pins its absence.
- **T0's capture** is kept in `captures/antigravity_permission_dialog.txt`; the test fixture
  `prompt::REAL_ANTIGRAVITY_DIALOG` is byte-identical to it.

## L2 Review Log
- **Round 1 (NEEDS_FIX, 10 issues), all verified against the code or the transcripts before
  acting:**
  1. D13's background test was a deny-list; a renamed or new background tool would read a later
     reply as `Waiting`. **Fixed:** allow-list of foreground tools. Re-measured: identical
     outcome on the corpus (56 / 55 / 0 false waits).
  2. D5 omitted D6's `Do you want to proceed?` and did not say whether markers pool across
     vocabularies. **Fixed:** per-vocabulary 2-of-3; question added.
  3. D7 did not define the level for a skipped scan or a departed agent. **Fixed** (revised in
     round 2 — see below): cleared when the agent goes; scroll-back handled by D7(a)/(b).
  4. D11 probed every tick while unresolved, and assumed a pid's conversation never changes.
     **Fixed:** at most one probe per TTL; the conversation assumption named as A1.
  5. D10's 8 KiB tail and D14's 1 MiB scan were unordered. **Fixed:** the tail classifies; the
     back-scan runs only after a tool-less reply. Measured: replies ≤ 6.8 KiB, none over 8 KiB.
  6. `STALE_LIMIT`'s outcome was unstated. **Checked in code:** the fallback returns `Idle`
     past it (`activity.rs:82-89`), never `Unknown`; stated in D1.
  7. "The band and the card cannot disagree" was claimed, not built. **Fixed:** one
     `effective_state` feeds both (D4); the bound on transient disagreement stated.
  8. D13's cost omitted the false `Ready` during background work. **Fixed:** Known Gaps.
  9. D12 had no owner or gate. **Fixed:** user-owned blocking pre-work; no fixture, no markers.
  10. 110 vs 111 replies. **Reconciled:** 110 with content + 1 empty.
- **Round 2 (NEEDS_FIX, 2 partial + 5 new), verified in code where it asked a question of
  fact:**
  - #3 D7 — named the update events; scroll-back keeps the last live value; returning to the
    bottom rescans once. (Clearing on scroll-back would reproduce the original bug: a silent
    agent at a dialog never redraws.)
  - #7 D4 — `push_agent_states` already runs on worker publications, PTY chunks and tab
    switches (`terminal_app.rs:1107, :1791, :2265`); stated, with the card's two triggers.
  - N1 closed set of result kinds + an "any other kind → no robust signal" row (D1).
  - N2 "No new idle work" reworded as bounded (the FD probe per TTL publishes nothing when
    unchanged). N3 A1 named as the one exception to the conservative bias.
  - N4 "last write (`mtime`)" in D1 and D13. N5 the trust marker is gated on its own capture.
  - Steelman (D4/D8, the screen outranks the log): the reviewer found D5's two-marker,
    per-vocabulary, exact-line rule answers it; no change.
- **Round 3 (NEEDS_FIX, 2 blocking, both introduced by the round-2 fix to #3):**
  - B1 — D7(b)'s rescan fed only the card. **Fixed:** it goes through the existing
    `push_agent_states_for(tab)`, which feeds the band in the same step; added to D4's list.
  - B2 — the scroll-back hold is a second exception to the conservative bias and changes the
    band for Claude too. **Fixed:** named in both Constraints.
  - Retry limit (2) reached; these two text fixes were applied without a fourth review.
- **L4 correction:** D12 said the existing `waiting-scan` line captures the dialog; it logs
  only the last six rows (`tab_scan_tail`). Found while checking T5's files; T0 widens it.
