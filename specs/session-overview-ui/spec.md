# Spec: session-overview-ui

## Meta
- **Created**: 2026-10-31
- **Type**: dev
- **Status**: approved
- **Approved by**: user (2026-10-31, after the unattended run stopped: "fix R4.1 and run agent-orchestrate")
- **Approved at**: 2026-10-31
- **Mode**: unattended
- **Branch**: `feat/changing-session-ui`
- **L2 review**: 3 rounds (9 + 4 + 3 issues, all fixed). Round 3's fixes (D2's bucket
  split, D9's `DialogSubject`, D6's `Agent` reach) were not re-reviewed by an L2
  reviewer — the retry budget was spent; the L4 Spec Review reads them cold.
- **Revises**: `specs/session-flow-graph/spec.md` (approved 2026-10-20, shipped in #56).
  Its decisions stay in force unless a decision below names one as superseded.
- **Reference design**: `specs/session-overview-ui/Agent Monitor — Overview.html` (moved
  here from the worktree root, where the user left it untracked; untracked still). It is a self-unpacking
  bundle with three boards: **Overview** (1440 × 940), **Compact sidebar** (380 × 900) and
  **Session detail — subagent timeline** (1440 × 900). Every colour, size and string
  quoted below was read from the unpacked board templates, not estimated from a picture.

## Goal
"I'd like to improve the session view UI. See the design (Agent Monitor — Overview.html)."
The user's list of what was wrong with the current screen and what the design changes:

1. **Session names were the conversation's first line** ("yes, delete that local branch
   t…", "Base directory for this skill: …"). Now the project name, the agent kind
   (Claude Code / Codex / Antigravity) and the branch are shown large; the user prompt is
   demoted to secondary text.
2. **Status did not stand out.** Four summary tiles on top: Needs you, Working, Idle, and
   subagents running/done. "Needs you" is yellow and strongest, and when a session waits
   for approval a banner jumps straight to it.
3. **Finished subagent cards took most of the screen.** Each subagent is now one small
   square on its row (running orange, done green, failed red), with a summary such as
   "2 running · 1 done"; a tree list only when expanded.
4. **No "what is it doing now".** A column for the last tool call or the pending approval,
   and an elapsed-time column.
5. **No sort or filter.** Auto-sorted Needs you → Working → Idle, with status filter tabs.
6. **The project card and the session card repeated the same information.** Merged into
   one row.

Plus two further boards: a **session detail** (below the overview) showing the session's
subagent runs as a horizontal bar timeline, a main-session lane split into working /
waiting-on-subagent / idle, a selected-subagent panel (type, start, duration, the first
part of its final report) and a recent-events list; and a **compact sidebar** (right,
380 px) meant to stay open beside the terminal. The design's start times, durations,
event log and report text are placeholders, to be replaced by "the values the real
monitor receives". Colours and type follow the app's existing dark background,
terracotta accent and monospace tone, not either "Design System" in the user's account.

`mode: unattended`.

## Non-goals
- **The compact sidebar board** (inferred — see D16). It needs a home this spec cannot
  settle without overriding a shipped decision: the right dock is the ai-monitor's alone
  (session-flow-graph D4 / architecture G10), and the left sidebar is the tab strip, whose
  order is the user's tab order and must not be regrouped by status. It is left to its own
  spec.
- **Codex, Gemini and Aider subagents.** Their logs are not read. (Antigravity was listed here
  until 2026-10-31; the premise "only Claude's log records subagents" was false — see D24.)
- **A "Needs you" state for dialogs the app cannot see.** Status stays the sidebar card's
  (one classifier); Codex's approval dialog is not matched by `prompt.rs` today and is not
  added here.
- **Animation.** The design's pulsing dots are drawn as static dots (session-flow-graph
  non-goal carried).
- **Free-form canvas interaction** (drag, zoom, pan) — carried.
- **Persisting** the filter, the expanded rows or the open view across app restarts.
- **The design's web fonts** (Inter, JetBrains Mono): the app draws with its own font set.
- The `updated just now` freshness text and the "Copy resume cmd" button's equivalents
  for non-Claude agents.

## Confirmed Goal
Replace the `Cmd+Shift+F` session graph's three-column card diagram with the design's
**Agent Monitor overview**, drawn by this app's renderer over the whole window exactly
where the graph is drawn today (a full-window overlay, no reflow, same open/close keys),
and add the design's **session detail** view, reached from the overview.

**Overview** (design board 1):
- A header `Agent Monitor` with `{n} sessions · {m} subagents`, and filter tabs
  `All / Needs you / Working / Idle`, each with its count.
- Four summary tiles: **Needs you** (yellow, strongest), **Working**, **Idle**, and
  **Subagents** (running / done counts and a proportion bar).
- A **banner** when any session needs the user, naming it and what it waits on, with an
  `Open session →` action that switches to that session's tab.
- One **row per agent session** (the graph's sessions: every detected agent of any kind,
  in a tab or tmux pane), columns *Status · Session · Now doing · Subagents · Elapsed ·
  expand*. Session = project name + agent-kind tag on line 1, branch on line 2. Now doing
  = the last tool call or the pending approval, with the latest user prompt underneath as
  muted secondary text. Subagents = one small square per subagent (running orange / done
  green / failed red) plus `N running · M done`; the expand control shows them as a tree
  list (type, description, status, duration).
- Rows are sorted Needs you → Working → Idle; the filter tabs restrict them.
- A legend line at the bottom.

**Session detail** (design board 3), opened by clicking a row:
- A header card: project, agent tag, status pill, branch, worktree path, elapsed,
  subagent counts, and a `Focus terminal` action (switch to the tab and close).
- A **timeline**: a main-session lane coloured working / waiting on subagent / idle, and
  one lane per subagent as a bar with its duration; clicking a lane selects it.
- A **selected subagent** panel (description, type, status, start offset, duration, first
  lines of its final report) and a **recent events** list.

**Done when** (each checkable in the running app):
1. With a Claude session in one tab and an agent in another, `Cmd+Shift+F` shows one row
   per session titled by **project name and agent kind**, not by the conversation's first
   line, and the Needs you + Working + Idle tile counts plus the number of rows with the
   grey `Agent` pill (unknown state) equal the header's `{n} sessions`.
2. A Claude permission dialog in any tab moves that row to the top as `Needs you`, shows
   the banner naming what it asks to run, and `Open session →` switches to that tab.
3. A session that has run subagents shows one square per subagent and a
   `N running · M done` summary, and its rows' tree appears only when expanded.
4. Clicking a Claude session's row opens its detail with a timeline whose bars come from
   the session log's real subagent start and end times, and `Esc` returns to the overview.

Inferences beyond the literal request (recorded, not asked — unattended):
- (inferred) The "session view" being redesigned is the shipped `Cmd+Shift+F` session
  graph; the overview **replaces** its three-column card diagram (project cards, edges and
  `N finished` chips go), keeping its overlay seam, keys, data sources and costs-only-while-
  open property.
- (inferred) The detail is a second view inside the same overlay, reached by clicking a
  row; clicking a row no longer switches tabs directly — `Open session →` and
  `Focus terminal` do.
- (inferred) The compact sidebar board is out of scope for this spec (Non-goals, D16).
- (inferred) "Elapsed" is the time since the session's latest human prompt (D7).
- (inferred) Design values marked as placeholders (`[file]`, `[subagent 6]`, the report
  text, the times) are replaced by data read from the session log, and where the log has
  no such data the cell reads `—` rather than inventing a value.

## Research

### The view being replaced (session-flow-graph, shipped in #56)
- Overlay: `FlowOverlay` (`crates/app/src/flow.rs:477`), registered through the SD4
  `Overlay` seam; `on_key` (`flow.rs:633`, `Esc` and the chord close, all else swallowed),
  `on_wheel` (`:674`), `on_press` (`:695`), `on_hover` (`:728`); priority 300 and full
  pointer ownership via `input::pointer_owner` (architecture G10).
- Pure policy: `build_graph(rows, metas, home, subagents, folds, active) -> (FlowGraph,
  FoldSet)` (`flow.rs:284`) over `sessions_of` (`flow.rs:~100`), which takes each session's
  card from `row_meta_with` (`crates/app/src/sidebar_model.rs:90`) — the sidebar card's own
  join, status via `resolve_status` (`sidebar_model.rs:246`) and `effective_state`
  (`:217`, a blocking dialog forces `Waiting`). Non-active tmux panes are log-only.
  Fold set keyed `(tab, pane pid)` with `0` for an unsplit tab (`flow.rs:57`).
- Rebuild key `RebuildKey { tabmeta, ready, rows_rev, active, subagents, folds }` checked in
  `poll` and once per frame in the layer's `record` (`flow.rs` `Core`, ~`:390-470`).
- Renderer: `graph_layout.rs` (1727 lines: `FlowGraph`, `FlowNode`, `layout_graph_with`,
  `compose`, slots, Bézier edges as capsule chains) and `graph_panel.rs` (977 lines:
  `FlowPanel` at z `GRAPH_PANEL_Z = 110`, two-stage cache, own `CardAtlases`, built on the
  first visible frame; `FlowHit::{Node, Fold}`).
- Tests that encode the old shape: `crates/app/tests/tt{1,2,3,4,5,6,7,8,9,11,14}_graph_*.rs`,
  `tt7_flow_telemetry.rs`; `crates/renderer/tests/tt{3,9,10,11,12}_graph_*.rs`.

### What each design field can be fed from
| Design field | Source today | Gap |
|---|---|---|
| project name | `TabMeta.repo: Option<RepoId>` (`crates/app/src/tabmeta/mod.rs:118`), resolved from the agent process's cwd while the graph is open; `main_root` basename | non-git session: folder of the cwd (`sidebar_model::folder_name`) |
| agent kind | `TabMeta.graph_agent` / `agent` → `Agent { kind: AgentKind, state }` (`mod.rs:181,241`): Claude, Antigravity, Codex, Gemini, Aider | none |
| branch | `RowMeta.branch` via `row_meta_with` | none |
| status | `RowMeta.status: CardStatus` (`crates/renderer/src/sidebar_card.rs:28`): `Working`, `Ready`, `WaitingForInput`, `Hidden` for agents | none |
| user prompt | `TabMeta.task` = latest substantive Claude prompt, one-line (`mod.rs:79`) | Claude only |
| now doing — last tool | `ai_monitor::telemetry::CompletedTool { name, ok, duration_ms, start_ms }` (`crates/ai-monitor/src/telemetry.rs:192`) | **no argument** (no command, no path) |
| now doing — approval | `prompt::shows_blocking_prompt(screen) -> bool` (`crates/app/src/prompt.rs:78`), published as `SidebarRow.blocking_dialog: bool` (`crates/renderer/src/sidebar_row.rs:83`, setter `:579`; the scan is `Session::rescan_dialog`
  `crates/app/src/session.rs:260`, and the row is patched by `patch_dialog_rows`
  `crates/app/src/terminal_app.rs:~3299`) | **no subject**: the dialog's command is on screen (`REAL_DIALOG` `prompt.rs:321`: the row under `Bash command`; `REAL_ANTIGRAVITY_DIALOG` `:345`: the row under `Requesting permission for:`) but never extracted |
| subagents | `Subagent { id, subagent_type, description, start_ms, end_ms, completed, ended_ok, … }` (`telemetry.rs:224`), followed per Claude log by `flow_telemetry` (`crates/app/src/flow_telemetry.rs`, slot `log → Vec<Subagent>` `:51`, publish `:250`), cap 32 | **no report text** |
| elapsed | `TelemetrySnapshot.first_event_ms`, `last_event_ms` (`telemetry.rs:355,359`) | **no turn boundaries** (latest human prompt time, turn ends) |
| recent events | `TelemetrySnapshot.tools`, `files: Vec<FileActivity { op, path, start_ms }>` (`:213`), subagents | turn-finished times |
| session id (resume cmd) | `TelemetrySnapshot.session_id` (`:324`) | none |

The flow telemetry thread already folds the **whole** Claude log into a `SessionTelemetry`
and publishes only `snapshot().subagents` (`flow_telemetry.rs:318`); the rest of the
snapshot is computed and discarded.

### Drawing
- Text faces: the panel draws through `CardAtlases` with two faces, `Title` (17 px bold)
  and `Meta` (12.5 px) (`crates/renderer/src/sidebar_text.rs:25,122`), built per scale
  by `Atlas::new(device, fonts, ppem)` (`crates/renderer/src/atlas.rs:274`). The design
  uses 34 px numerals in the tiles; no face that size exists.
- Shapes: `SidebarShape` rounded boxes, capsules and rings (`sidebar_shapes.rs`); icons
  `Icon::{GitBranch, Pause, Check, Cross, …}` via `push_icon` (`sidebar_icons.rs:17,261`).
  No scissor rect: lists scroll in whole rows (domain.md "slot", "item ladder").
- Clipboard writes exist (`crates/app/src/copy.rs`, `clipboard.rs`).

### Rules that bind the change
- `renderer` stays app-type-free (tab ids as `u64`) and winit-free; `term-core` untouched.
- G1: panes, repositories and subagents cost nothing while the overlay is closed.
- CLAUDE.md: a render-path change is covered only by a `Renderer::capture` pixel assertion
  (`crates/renderer/tests/golden_frame.rs`); new pure logic is judged by `cargo mutants`
  survivors (0 is the bar the last files were brought to).
- Build/test/lint: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`.

## L2 Checkpoints

Complexity: **Complex (5 signals)** — external resources (Claude session `.jsonl` logs,
the PTY screen for dialog text, git metadata via the worker); 3+ subsystems (tabmeta
worker, flow telemetry thread, ai-monitor telemetry fold, screen dialog matcher, renderer
layout/panel, overlay input); async (worker + telemetry thread); multi-stage state
(closed / overview / detail, filter, expanded rows, selected lane); data shape (a per-log
digest replacing `Vec<Subagent>`). Brownfield weights: Core 30 · Scope 20 · Error 25 ·
Data 15 · Implementation 10.

| Dimension | Checkpoints | Resolved by |
|-----------|-------------|-------------|
| Core | what replaces the graph; row identity and title; status buckets and sort; filter; tiles; banner and its action; now-doing text; elapsed; subagent squares and tree; detail entry/exit; timeline window and segments; selected panel; events list; header actions | request (D1, D3, D4, D5, D6, D8, D10), assumed (D2, D7, D9, D11, D12, D13) |
| Scope | which sessions; non-Claude rows; compact sidebar; old graph features retired; panes | carried (D2 of session-flow-graph R14), assumed (D3, D14, D16) |
| Error/Edge | unknown status; missing log / telemetry; dialog subject not extractable; narrow window; many rows; many subagents; non-Claude detail; time text going stale; session vanishing while its detail is open; window below the table's minimums; background work outstanding past `end_turn`; sliding-window time labels; a press on a session that vanished; state kept across reopen | assumed (D4, D7, D9, D12, D15, D17, D18) |
| Data | per-log digest shape; tool argument summary; report preview; turn boundaries; dialog subject publication; model handed to the renderer | assumed (D7, D9, D11, D19, D20) |
| Implementation | layout/panel module plan; text faces; palette; cache key; tests and retirements | L1 + assumed (D15, D21, D22, D23) |

Fenced checkpoints: none — no schema, migration, auth, payment or secret is touched (the
resume command copied to the clipboard is a session id the user's own log already holds).
Composite 1.00 counting `assumed` as resolved. Inversion probe: *every requirement met
yet it fails* if the overview titles a row by project but two sessions of one project are
indistinguishable — two Claude tabs in one repository, same branch. Line 2 therefore
carries the worktree directory when it differs from the main root, and the prompt line
distinguishes the rest (D3). Implication probe: "the banner jumps to the waiting session"
also means deciding what it does with two waiting sessions (D5).

## Decisions

Carried unchanged from `specs/session-flow-graph/spec.md`: **D3**'s status join (a
session's state is its sidebar card's, through `row_meta` / `resolve_status`; non-active
panes are log-only), **R14** (a session is any detected agent of any kind), **D5**'s keys **as amended by D10 here**
(`Cmd+Shift+F` toggles; while open the overlay owns keys, presses, wheel and hover; `Esc`
closes from the overview but returns to the overview from the detail),
**D4**'s placement (full-window, z 110, no inset, covers an open ai-monitor, never
restored open), **D10** (repository identity from the agent process's cwd, published only
while open, per-open epoch), the subagent thread's lifecycle (old D10: spawned on open,
stopped and never joined on close) and its cap of 32 per log, and the `graph_agent` /
`graph_task` toggle-free agent. **Superseded**: D1 (three-column graph), D2 (project
card), D7 (click targets), D8's presentation (`N finished` chips — the fold *set* becomes
the expanded-row set, D10 here), D12/D13 (slot layout and narrow fallback), D17
(verification shape). Old names (`FlowOverlay`, `graph_panel`, `GRAPH_PANEL_Z`) may stay
or be renamed by the implementer; behaviour, not names, is specified.

### D1: The overview replaces the graph inside the same overlay
- **Status**: resolved (request + inferred scope)
- **Rationale**: The user's "session view" is the shipped `Cmd+Shift+F` graph, and every
  problem they list (titles from the first line, project/session duplication, finished
  subagent cards) is a property of that graph. The overlay seam, keys, pointer ownership,
  z-order, data sources and close-time teardown are kept; only the model and its drawing
  change. Rejected: a second overlay beside the graph (two views of one thing, two chords),
  and drawing the overview inside the ai-monitor dock (380–600 px cannot hold the 1060 px
  table the design specifies).

### D2: Status buckets, sort and counts
- **Status**: assumed (revised after the third L2 review)
- **Rationale**: The card's `CardStatus::WaitingForInput` does **not** mean "needs you":
  `activity::state_from` (`crates/app/src/tabmeta/activity.rs:72`) maps every plain
  assistant `end_turn` to `AgentState::Waiting`, so every Claude session that has merely
  finished its turn reads `WaitingForInput`. Bucketing on it alone would paint almost
  every idle Claude session yellow, keep the banner up permanently and never show the
  design's Idle row — the opposite of request item 2. The bucket is therefore the card's
  status **split by why it waits**, from two signals the app already computes and the
  overview does not re-derive:
  - **Needs you**: `WaitingForInput` **and** either the row's `blocking_dialog` is true
    (the screen shows a permission, question or plan dialog — Claude or Antigravity), or
    the session's log's last meaningful message is `LastMessage::AssistantAsksUser`
    (Antigravity's `ask_question`). The worker publishes the latter as a graph-only
    `TabMeta.graph_asks_user: bool`, set on graph passes only, like `graph_agent` (G1).
  - **Idle**: `Ready`, or `WaitingForInput` with neither signal (a finished turn).
  - **Working**: `Working`.
  - **Agent**: `Hidden` (agent present, state unknown). Muted pill `Agent`, counted in
    `All` and `{n} sessions`, in none of the three tiles, sorted after Idle: an unknown
    agent must never render as ready (`AgentState` doc, `tabmeta/mod.rs:205`).
  A **non-active tmux pane** takes the log-only status (carried); the dialog matcher sees
  only the active pane, so such a pane is Needs you only through `AssistantAsksUser`, published per pane as
  `PaneMeta.graph_asks_user`, and a
  finished turn there is Idle. The sidebar card keeps its own wording (`Waiting for input`
  after any turn); the overview's bucket is a refinement of that one status by a published
  flag, not a second classifier, so the "classifies nothing itself" constraint holds.
  Sort key: bucket (Needs you 0, Working 1, Idle 2, Agent 3), then the graph's session
  order (strip order, then pane order), stable so rows do not shuffle within a bucket.
  Rejected: bucketing on `WaitingForInput` alone (above); changing `state_from` to split
  `Waiting` (it would change the sidebar card and the ai-monitor band, outside this view);
  sorting by elapsed inside a bucket (rows would move every minute under the pointer).

### D3: A row's identity — project, agent kind, branch, prompt
- **Status**: resolved (request) + assumed (fallbacks, disambiguation)
- **Rationale**: Line 1: **project name** in the title face, then an **agent tag** with
  the kind's label — `Claude Code`, `Codex`, `Antigravity`, `Gemini`, `Aider` — in the
  kind's colour. Project name = basename of `RepoId.main_root`; a session outside any
  repository uses the folder name of its agent's cwd (`folder_name`). The agent's cwd is the
  one the worker already reads to resolve `repo` (D10); it is published on graph passes as
  `TabMeta.graph_agent_cwd` / `PaneMeta.graph_agent_cwd` (T3), not taken from the shell's
  `cwd`, which can differ after a `cd` (G5). Before the worker's
  first full pass for this open (`graph_ready`), D17's `resolving…` rule applies and no row
  is drawn at all; after it, a session that appears later (a tab opened while the overlay
  is open) and has no `repo` yet shows its folder name until a pass resolves it. Line 2: the
  git-branch icon and the branch; when the session runs in a **linked worktree**, the
  worktree directory's basename follows the branch after ` · ` (the design's detail header
  shows `…/terminal-fix-mouse-drag-not-working`), which is also what tells two same-project
  rows apart. No branch → line 2 shows the `~`-abbreviated cwd. The **user prompt**
  (`TabMeta.task` via the toggle-free `graph_task`) moves to the Now doing column's second
  line, in quotes, muted (`#8F887F`); no prompt → that line is omitted. The conversation's
  first line is no longer a title anywhere in this view. Rejected: keeping `RowMeta.line1`
  as title (the problem the user reported) and showing the full cwd on line 1 (too long
  for the 240 px column).

### D4: "Now doing" text, by state
- **Status**: resolved (request) + assumed (exact strings, sources)
- **Rationale**: First line, mono, coloured by bucket (Needs you `#F2C76B`, Working
  `#EDE8E1`, Idle `#A39C92`):
  - **Needs you**, by D9's subject: `Command(c)` → `Approval needed · {c}`;
    `Question` → `Question waiting`; `Plan` → `Plan approval waiting`; none, or an
    Antigravity `ask_question` from the log → `Waiting for your input`.
  - **Working** with k ≥ 1 running subagents, when the digest's newest finished tool
    that is **not** an `Agent` / `Task` call (D11's `last_work_tool`) is absent or has
    `start_ms ≤` the newest running subagent's `start_ms` (equal counts as waiting):
    `Waiting on {k} subagent(s)` (`subagent` when k = 1).
  - **Working** otherwise: `{Tool} · {summary}` of the log's newest finished tool call
    (D11), or `{Tool}` when it has no summary, or `Working` with no tool seen.
  - **Idle**: `Turn finished · ready for next prompt`.
  - **Agent** (unknown): `Agent running`.
  Codex, Gemini and Aider sessions have no log digest, so they use the state-only strings
  (Antigravity has one since D24). The
  design's `⏸` glyph is dropped: the font chain does not guarantee it, and a tofu box in
  the strongest cell is worse than no glyph. Claude Code writes a `tool_use` only once it
  returns (G8), so "the last tool call" is honestly the last **finished** one; the text
  says what was last done, not what is in flight. Rejected: reading the in-flight call
  from the screen (a second screen parser per frame for every tab).

### D5: The banner
- **Status**: resolved (request) + assumed (several waiting, action)
- **Rationale**: Shown iff ≥ 1 row is Needs you, between the tiles and the table:
  `Waiting on you` (bold, `#F2C76B`), then `{Agent} · {project} — {now-doing text}` of the
  **first** Needs-you row in sort order, then ` (+{k} more)` when k others wait, then an
  `Open session →` button (`#F2B84B` fill, `#2A2110` text). The button and the
  ` (+{k} more)` suffix are never clipped; when the line is too long, the now-doing part
  is ellipsised first, then the project name. Pressing the button switches
  to that session's tab (and, for a non-active pane, that tab only — panes are not
  focusable from here, carried) and closes the overlay — "바로 이동" read as reaching the
  session, where the approval can actually be given; scrolling to the row would leave the
  user one more step from approving. The banner ignores the filter (it is about the user,
  not the list).

### D6: Filter tabs
- **Status**: resolved (request) + assumed (persistence, keys)
- **Rationale**: `All {n}`, `Needs you {q}`, `Working {w}`, `Idle {i}` in the header's
  pill group; pressing one selects it (selected: `#3A3531` fill, `#F4EFE8` text). The
  filter restricts table rows only, never tiles or banner. `Agent` rows (D2) have no tab
  of their own and are reachable only under `All`, which is rare enough not to earn a
  fifth tab. Empty result → `No sessions in
  this filter.` centred in the table. The filter is kept across close/reopen within one app
  run and resets to `All` at startup (Non-goals: no persistence). Mouse only; no key
  binding is added, because the overlay swallows typing and adding letter keys there is a
  separate interaction decision. Selecting a filter resets the table's scroll to the top.

### D7: Elapsed
- **Status**: assumed
- **Rationale**: The time since the session's **latest human prompt** (the same "human
  prompt" the background-work rule uses: a `user` record that is not a tool result, not
  `isMeta`, and not a task notification or peer message — G8), formatted `<1m` under a
  minute, `{m}m` under an hour, `{h}h {m}m` beyond — never seconds, so a minute-granular
  rebuild (D18) can never show a stale value. It pairs with the prompt shown in the
  same row: "this ask has been going for 12m". The design's numbers fit it: its idle
  `terminal` session reads 41m with one 36-minute turn on a 42-minute timeline. Sessions
  with no digest (non-Claude, or Claude with no human prompt yet) show `—`. Because the
  text depends on the clock, the model is rebuilt when the **minute** changes (a
  `now / 60 s` term in the rebuild key) — not every frame. Rejected: session age from
  `first_event_ms` (a resumed week-old session reads `168h`), and time in current state
  (needs state-change history the app does not keep across opens).

### D8: Subagent squares, summary and tree
- **Status**: resolved (request) + assumed (overflow, default)
- **Rationale**: One 12 × 12 logical px square per subagent, radius 3, gap 3, wrapping
  inside the 220 px column, in **log order**: running `#E8744F` (`!completed`), done
  `#6FA67F` (`completed && ended_ok`), failed `#E5534B` (`completed && !ended_ok`). Under
  it `{r} running · {d} done` (the `{r} running · ` part only when r > 0; failed count
  into done, as the design's summary does, and are told apart by colour). At most two
  rows of squares; beyond that the last square is replaced by `+{k}`. No subagents → `—`.
  The expand control (a chevron button, 44 × 44 logical px) exists only on rows with
  subagents; expanded, the row grows a tree: per subagent, a connector tick, a status dot,
  the type (`subagent_type`, else `agent`), the description (else `—`), the status word
  (`running` / `done` / `failed`) and the duration: a finished subagent's `end − start` as `{m}m {ss}s` (fixed once it
  ends, so never stale); a running one's `now − start` in D7's minute format (`<1m`,
  `{m}m`); `—` when unknown. Rows start **collapsed**; the expanded set is keyed and
  pruned exactly like the old fold set `(tab, pane pid)`.

### D9: The dialog subject
- **Status**: assumed
- **Rationale**: `prompt.rs` gains a pure `blocking_subject(screen: &str) -> Option<DialogSubject>`
  run on the same tail, at the same moment, as `shows_blocking_prompt`, and only on the branch
  that actually scans and matches (`next_dialog_level`, `session.rs:444`, returns a held level
  without scanning while the view is scrolled back; the subject is then held unchanged too),
  with `DialogSubject::{Command(String), Question, Plan}`:
  - Claude permission dialog → `Command`: the first non-blank row after the tool-title row (`Bash
    command`, `Edit file`, …) that follows the `────` rule, trimmed (`REAL_DIALOG` →
    `ls -d ~/.config`).
  - Antigravity → `Command`: the first non-blank row after `Requesting permission for:`.
  - `AskUserQuestion` dialog or its review screen → `Question`; `ExitPlanMode` approval →
    `Plan`.
  - Anything else matched: `None`.
  It is published beside the boolean as `SidebarRow.blocking_subject: Option<DialogSubject>`,
  set through the same setter path so the two cannot disagree (subject is `None` whenever
  `blocking_dialog` is false), and a subject change at an unchanged level is reported as a
  change, like a level change. A `Command` is truncated to 80 chars. The sidebar card does not draw it.
  Rejected: reading the subject from the log (not on disk while the dialog is up, G8).

### D10: Overview → detail → terminal
- **Status**: resolved (request: detail is reached from the overview) + assumed (gestures)
- **Rationale**: Pressing a row anywhere outside its two buttons opens that session's
  **detail** in the same overlay. Every row also carries an **open** button (`→`, 44 × 44
  logical px) in its last column, beside the chevron: pressing it switches to the
  session's tab and closes the overlay — one click, as the old graph's card click was.
  The last column therefore widens from the design's 52 px to 104 px (D15). This keeps
  the old "glance, then jump" path one click long for every session, not only the
  waiting one the banner names, while a plain row press reaches the detail. In detail: `Esc` returns to the overview (scroll,
  filter and expanded set preserved); `Cmd+Shift+F` closes the overlay from either view;
  the breadcrumb `‹ All sessions` returns too; `Focus terminal` switches to the session's
  tab and closes. The overview's `Esc` closes as today. Opening the overlay always shows
  the overview. A session that disappears while its detail is open (tab closed, agent
  exited) returns the view to the overview on the next rebuild — no stale detail.
  A press whose `(tab, pane)` no longer exists when it lands (closed between the last
  rebuild and the press) does nothing and leaves the overlay open. Across a close and
  reopen within one app run the expanded set is kept (pruned on rebuild like the filter
  is kept, D6); the view resets to the overview and every scroll offset to the top.
  Rejected: a click switching tabs directly (the old behaviour; it leaves no way to reach
  the detail), a modifier-click to jump (undiscoverable, and the overlay's press path
  carries no modifiers today), and a split overview+detail on one screen (the 1440 px design boards are
  separate screens).

### D11: Last-tool summary in telemetry
- **Status**: assumed
- **Rationale**: `ai_monitor::telemetry` records, per finished tool call, a one-line
  `summary: Option<String>` from its `tool_use` input: `Bash` → `command`; `Read` /
  `Edit` / `Write` / `NotebookEdit` → `file_path` basename; `Grep` / `Glob` → `pattern`;
  `Agent` / `Task` → `description`; `WebFetch` → `url` host; `WebSearch` → `query`;
  `Skill` → `skill`; other tools → `None`. First line only, whitespace collapsed,
  truncated to 80 chars with `…`. The snapshot exposes the newest finished tool as
  `last_tool: Option<(String, Option<String>, Option<u64>)>` (name, summary, start_ms) —
  and `last_work_tool` of the same shape, the newest finished tool whose name is neither
  `Agent` nor `Task` (D4's comparison). The ai-monitor panel keeps reading `tools`
  unchanged. Rejected: storing full inputs
  (memory grows with session length, and commands can carry secrets the view never needs
  beyond 80 chars).

### D12: The detail timeline
- **Status**: resolved (request: lanes, segments, click) + assumed (window, ticks)
- **Rationale**: Window: **the last 60 minutes**, or from the session's first event if
  that is more recent, to now; ticks at whole 10-minute marks of the wall clock, labelled
  local `HH:MM` — a sliding window must not relabel a fixed event every minute. Lanes: one per subagent whose `[start, end or now]` overlaps the
  window, oldest first, clipped at the window edges; label `└ {description}`, bar
  `#6FA67F` (done), `#E8744F` (running), `#E5534B` (failed), the selected one `#8FC29E`,
  duration text right of the bar. The **main session** lane is split into: *idle*
  (`#3A3632`) from each turn end to the next human prompt, *waiting on subagent*
  (`#6B3A2A`) where a turn is in progress and at least one subagent is running, and
  *working* (`#E8744F`) for the rest of a turn. Turn boundaries come from the digest
  (D19); a part of the window older than the oldest kept turn is drawn blank (the
  lane's track colour), not idle. A turn ends only when the main agent has ended its turn **and** no
  background work started in it is still pending — the very rule the card's G8 Working
  state uses — so a lane never reads idle while the status pill reads Working, and a
  background agent's run shows as *waiting on subagent*. A non-Claude session's detail shows the header and, in place of the timeline,
  `No timeline — this agent's log is not read.` Rejected: the whole session
  as the window (a long session makes every bar a hairline) and only the latest turn (a
  short follow-up prompt would empty the timeline the user came for).

### D13: Selected subagent panel and recent events
- **Status**: resolved (request) + assumed (default, count, strings)
- **Rationale**: Clicking a lane selects it; default selection is the **newest** subagent
  in the window. Panel: description (title face), then `Type`, `Status` (● in the status
  colour), `Started` (local wall-clock `HH:MM:SS`), `Duration`, then the **first
  four lines** of the subagent's final report (D19), `—` when none was recorded. Recent
  events: the newest **8** of, newest first, local wall-clock `HH:MM` + text: `Turn finished`, `Task start · {description}`,
  `Task done · {description}` (`#8FC29E`) / `Task failed · {description}` (`#E5534B`),
  `{Tool} · {summary}` for finished tools. Only events inside the window.

### D14: Which rows exist
- **Status**: carried + assumed
- **Rationale**: Exactly the graph's sessions (session-flow-graph R14 and its pane rule):
  every tab or tmux pane with a detected agent of any kind, in this window. Plain shells
  are not rows. Project grouping is gone: the project is a field of the row (request item
  6), so there are no project cards, no unparented tail and no edges.

### D15: Layout, sizes and the narrow window
- **Status**: assumed (from the design, scaled)
- **Rationale**: Logical px, multiplied by the window scale. Outer padding 28/32; content
  width `min(viewport − 64, 1376)`, centred. Header row; tiles in a 4-column grid, gap 12,
  height 104; banner 48; table header 40; row 72; tree line 32; legend 28. Table columns:
  Status 120 · Session `minmax(240, 1.2fr)` · Now doing `minmax(280, 2fr)` · Subagents 220
  · Elapsed 88 · buttons 104 (chevron + open, D10), gap 16, padding 20. Thresholds are the sum of the remaining minimums, inner gaps
  (16 each) and row padding (2 × 20), against the content width W:
  - **W ≥ 1172**: every column as above; spare width goes to Session and Now doing 1.2 : 2.
  - **1052 ≤ W < 1172**: Now doing's minimum drops to 160.
  - **920 ≤ W < 1052**: Subagents narrows to 88 (squares only, six per line, no summary
    text); the tiles wrap to 2 × 2.
  - **816 ≤ W < 920**: the Elapsed column is dropped.
  - **712 ≤ W < 816**: the Subagents column is dropped too (the chevron stays in the
    buttons column).
  - **W < 712**: the view draws only the header and `Window too narrow — widen it to see
    sessions.`
  Text is always
  ellipsised, never overlapping. The table scrolls by wheel in whole lines (a row
  or a tree line), since there is no scissor (carried rule); the header, tiles, banner and
  filter stay fixed. Detail: two columns (timeline flexible, side panel 340) when the
  content is ≥ 1000 px wide, else stacked with the side panel below; lanes are 40 px and
  scroll the same way.

### D16: The compact sidebar board is a separate spec
- **Status**: assumed (flagged for the person)
- **Rationale**: The board is "a narrow version to keep open on one side" of the screen.
  The only right-side surface is the ai-monitor dock, which session-flow-graph D4 returned
  to the monitor alone after the shared dock was tried and reverted; the left surface is
  the tab sidebar, whose card order is the tab order. Either home reverses a shipped
  decision, which is a product call, not one to take unattended. The overview's model
  (D2's buckets, D4's text, D8's squares) is built so a later compact view can reuse it.
  Recorded in Known Gaps.

### D17: Degradation
- **Status**: assumed
- **Rationale**: Missing log or digest → the row shows state-only now-doing text, `—`
  elapsed, `—` subagents. Repository unresolved → `resolving…` replaces the header's
  counts and no row is drawn (carried D16 behaviour). No agent sessions → tiles read 0,
  the table shows `No agent sessions in this window.` A font lacking a glyph falls through
  the existing fallback chain. Telemetry parse errors already count, unchanged.

### D18: Time text freshness and cost
- **Status**: assumed
- **Rationale**: The rebuild key gains a `minute: u64` term (`now_ms / 60_000`) and the
  digest generation replaces the subagent generation. Stage two (compose) runs on scroll,
  hover, filter, expand, view and selection changes; an unchanged frame writes no buffer
  (G1, carried). Nothing new runs while the overlay is closed — the digest is folded only under D19's
  flag, which the ai-monitor's fold never sets — with one bounded
  exception, D9's `blocking_subject`: it runs where `shows_blocking_prompt` already runs,
  only on a tail that matched, over the same 20 rows, so a dialog's subject is already
  known when the overlay opens. A change of subject at an unchanged boolean counts as a
  row change (`set_blocking_dialog` reports `changed`), so the open overlay rebuilds.

### D19: The per-log digest
- **Status**: assumed
- **Rationale**: The flow telemetry thread publishes `log → SessionDigest` instead of
  `log → Vec<Subagent>`: `subagents` (as today), `last_tool` (D11), `last_prompt_ms`,
  `turns: Vec<Turn { start_ms, end_ms: Option<u64> }>` (start = a human prompt; end = the first
  moment after it at which the assistant has ended its turn and no background work
  started since that prompt is pending, decided by the same start/completion records
  `ai_monitor::activity` uses for G8's Working rule — one rule, not a restatement;
  a task-notification record for that background work **resumes** the turn — the main
  agent answers it — so the turn then ends at the next `end_turn` with nothing pending;
  `None` while either is outstanding; the newest 64 kept), `events` (D13's source, the
  newest 64), `first_event_ms`, `session_id`, and per subagent a `report_preview:
  Option<String>` — the first 4 non-blank lines (≤ 400 chars) of the `Agent` result text,
  or of the `<result>` of its task notification for a background agent. All folded in
  `ai_monitor::telemetry` alongside what it already folds, so the ai-monitor and the
  overview read one parser — but **opt-in**: a `SessionTelemetry` folds the digest
  additions (D11's summaries, `last_prompt_ms`, `turns`, `events`, report previews) only
  when built with the digest flag, which only the flow telemetry thread sets. The
  ai-monitor's own fold leaves the flag off: it does no new work, its snapshot's new
  fields stay empty, and its draw cache key is unchanged. Rejected: a second parser in the app crate (two readings of
  one log drift — the reason old D2 forbids a second classifier), and folding the digest
  unconditionally (every ai-monitor user would pay, while the overlay is closed, for data
  only the overlay draws).

### D20: The renderer model
- **Status**: assumed
- **Rationale**: The app hands the renderer plain data, `u64` tab ids:
  `OverviewModel { resolving, sessions: Vec<SessionRowData>, filter, expanded, view }`
  where `SessionRowData` carries every string already formatted (project, agent label,
  agent colour index, branch line, now-doing, prompt, elapsed, status bucket, subagent
  list with status/type/description/duration text) plus the target `(tab, pane)`; and
  `view = Overview | Detail(DetailData)` with lanes, segments, ticks, events and the
  selected index already in window fractions. All formatting and policy are app-side pure
  functions (testable without a GPU); the renderer lays out and draws. Hits are a closed
  enum: `Filter(bucket)`, `Row(tab, pane)`, `Expand(tab, pane)`, `Open(tab, pane)`,
  `Banner(tab, pane)`, `Back`, `Focus(tab, pane)`, `Lane(index)`, `CopyResume`. Every
  session-addressed hit carries the same `(tab, pane)` identity; the three jump hits
  (`Open`, `Banner`, `Focus`) switch to `tab` only, since panes are not focusable from the
  overlay (carried).

### D21: Text faces and palette
- **Status**: assumed
- **Rationale**: Three faces in the panel's own atlases (not new `CardFace` variants — the
  sidebar builds no extra atlas): `Title` (17 px
  bold) for the view title, project names and the selected-subagent title; `Meta`
  (12.5 px) for everything else; and a new **`Display`** face at 30 logical px for the
  four tile numbers only (the design's 34 px, rounded to keep the tiles at 104 px). The
  design's 11 px uppercase labels use `Meta`. The design's colours are used as given and
  live in one palette module: background `#1A1816`, panel `#211F1C`, card `#24221F`,
  border `#36322E`, text `#EDE8E1`, muted `#A39C92`, accent `#E8744F`, waiting `#F2B84B`
  / `#F2C76B` (the Needs-you tile's fill is the design's `rgba(242,184,75,.10)` over the
  background, drawn opaque as `#30271B`; its border `rgba(242,184,75,.45)` as `#7B602E`), done `#6FA67F`, failed `#E5534B`; agent tags Claude `#E9967A`, Codex
  `#A9B8FF`, Antigravity `#86D1C4`, Gemini and Aider `#B8B1A7` (the design defines none).

### D22: Header actions in detail
- **Status**: resolved (design) + assumed (scope)
- **Rationale**: `Focus terminal` (accent fill) always; `Copy resume cmd` only for a
  Claude session with a known `session_id`, writing `claude --resume {session_id}` to the
  clipboard through the existing copy path.

### D23: Verification shape and retirements
- **Status**: assumed
- **Rationale**: Policy (buckets, sort, filter, now-doing, elapsed formatting, squares,
  banner choice, timeline segments, events) is app-side pure functions with unit and
  acceptance tests; the dialog subject is tested on the real captured fixtures in
  `prompt.rs`; telemetry additions on fixture logs in `crates/ai-monitor/tests`. The
  renderer layout is tested headless (rects, hits, ellipsis, narrow fallbacks), and the
  render path by a `Renderer::capture` golden-frame assertion that the Needs-you tile's
  pixels are the waiting colour and that a row's squares are drawn in their status
  colours. Old graph tests that assert removed behaviour (columns, edges, project cards,
  `N finished`) are deleted with the code they cover; tests of carried behaviour (keys,
  pointer ownership, monitor coverage, repository resolution, agents, telemetry lifecycle)
  are kept and re-pointed. New pure files reach 0 `cargo mutants` survivors or record each
  survivor as equivalent.

### Amendment 2026-10-31 (after the user's first run; approved by the user in the verification loop)

The user tested the build and reported three defects the gates did not catch. D24–D26 are
`resolved (user 2026-10-31)`; they amend D3, D7, D8, D12 and the Non-goals where named.

### D24: Antigravity sessions get a digest too (amends Non-goals, D7, D8, D12)
- **Status**: resolved (user 2026-10-31)
- **Rationale**: `ai_monitor::telemetry` already folds an Antigravity transcript's
  `invoke_subagent` calls into `Subagent`s (`telemetry.rs` `apply_antigravity`), and its
  `USER_INPUT` steps are the human prompts. The worker already memoises each `agy` pid's
  transcript path (`tabmeta/antigravity.rs`). So on **graph passes only** the worker
  publishes that path as `TabMeta.graph_transcript` / `PaneMeta.graph_transcript`; the
  overlay's digest thread follows it with `SessionTelemetry::with_format(LogFormat::Antigravity)
  .with_digest()`; and the digest for an Antigravity transcript carries `subagents`,
  `first_event_ms`, `last_prompt_ms` (the newest `USER_INPUT` step), `last_tool` /
  `last_work_tool` (tool name; summary `None` unless the call's args map as in D11), `events`
  (tool calls, subagent start/done/failed) and `turns` (start: a `USER_INPUT` step; end: the
  first tool-less planner response after it with no background work pending — the rule
  `agy-background-working` already uses for the card). An Antigravity row then shows squares,
  summary, tree, elapsed and a detail timeline exactly as a Claude row does. `Copy resume cmd`
  stays Claude-only. The detail's no-timeline message becomes `No timeline — this agent's log
  is not read.` (Codex, Gemini, Aider). Rejected: reading Antigravity subagents from the screen
  (the transcript already has them).

### D25: The prompt line and the tab label are human prompts only (amends D3)
- **Status**: resolved (user 2026-10-31)
- **Rationale**: `ai_monitor::activity::recent_prompts` accepted every `user` record with no
  `toolUseResult` — task notifications, peer messages and `isMeta` skill injections
  ("Base directory for this skill: …") included — so the row's prompt line (and the sidebar
  card's title, which reads the same label) showed `<task-notification> …`. It now admits only
  records `activity::is_human_prompt` accepts, the same predicate D7's elapsed and D19's turns
  use. One definition of "human prompt" for label, elapsed and turns.

### D26: The prompt is quoted once (fixes D3's rendering)
- **Status**: resolved (user 2026-10-31)
- **Rationale**: the model already carries `“…”` (`overview.rs`); the layout wrapped it again,
  drawing `““…”`. The layout now ellipsises the text **inside** an existing pair of quotes and
  draws one pair.

### D27: A handed-back task-notification is not a prompt (amends the card classifier)
- **Status**: resolved (user 2026-11-01, second manual run)
- **Rationale**: Claude Code 2.1.28x delivers a background subagent's report twice — a `peer`
  user record (`origin.handback: true`, `origin.from: <task id>`), which the assistant answers,
  then a `task-notification` user record for the same id, appended after that turn ended and
  never answered. `activity::classify_user` read the trailing notification as `UserPrompt`, so
  the card (and the overview row through `row_meta`) read Working indefinitely. Measured
  2026-11-01T00:17Z over `~/.claude/projects`: 22 of 24 notifications preceded by a hand-back for
  their id were followed by a human prompt or nothing; 2 of 249 without one. `activity::
  state_from_reads` now, only when the tail says `UserPrompt`, walks the `PROMPT_SCAN_CAP` window
  and skips trailing notifications whose id a hand-back names, classifying the message before
  them. A notification with no hand-back stays a prompt. Accepted cost: in the rare case Claude
  does answer such a notification (2 of 24), the card reads the previous `end_turn` until the
  answer is written.

## Constraints
- `term-core` is not touched; `renderer` gains no `winit` or app type; no GUI crate.
- Status is the sidebar card's, through `row_meta` / `resolve_status`; the overview
  classifies nothing itself — D2's bucket only splits `WaitingForInput` by two published
  flags (`blocking_dialog`, `graph_asks_user`).
- Nothing new runs while the overlay is closed (G1), with one stated exception (D18):
  the digest is built only by the thread that exists only while it is open, and the
  ai-monitor's own telemetry fold does no new work (D19's flag stays off there);
  `blocking_subject` runs only where `shows_blocking_prompt` already runs, and only on a
  tail that matched.
- No invented values: every number and string drawn comes from the log, the screen, the
  tab metadata or a fixed label; absent data reads `—`.
- macOS-only, local zsh only (project rules).
- An unchanged frame writes no GPU buffer (G1, two-stage cache carried).

## Known Gaps
- **D16 / compact sidebar**: the 380 px board is not built. Its home (left sidebar
  restyle, right dock shared with the ai-monitor, or a third surface) needs the person's
  decision; this spec's model is reusable by it.
- Codex, Gemini and Aider permission dialogs are not detected, so those sessions reach
  Needs you only through their log state, and their now-doing text is state-only (D4).
- A permission dialog in a **non-active** tmux pane is invisible to the dialog matcher, so
  that pane reads Idle (or Working) in the overview until it becomes the active pane (D2).
- Codex, Gemini and Aider sessions have no subagents, elapsed or timeline (D7, D12; their logs
  are not read — Antigravity gained them in D24).
- The dialog subject (D9) rests on two more strings of other programs' UIs — the tool-
  title row under the rule (Claude) and `Requesting permission for:` (Antigravity). A
  change reads as no subject (`Waiting for your input`), never as a wrong one. Add to
  architecture G8 with the other screen strings.
- **Worktree suffix from the agent's cwd** (found by the verification gate, 2026-10-31): D3's
  line-2 suffix is the basename of the agent process's cwd, not of the linked worktree's root,
  so an agent launched in a subdirectory of a linked worktree shows the subdirectory's name.
  The fix is to carry the worktree root on `RepoId` (it is known in `git.rs`'s
  `resolve_dot_git` as `dir`), which changes `RepoId`'s literal shape and so the acceptance
  tests that build one — left for a follow-up rather than edited inside the gate.
- **Project-name last resort** (D3): with no repository and no agent cwd, the row's project is
  the tab's own title (`SidebarRow.title`, `overview.rs` `identity`) — tab metadata, not an
  invented value, but D3 did not name the case.
- Elapsed (D7) and the timeline window (D12) are assumptions about what is most useful;
  they are cheap to change once used.

## Requirements

Boundaries named in the GWTs below that do not exist yet are created by the tasks in
`## Tasks`: the app-side policy module `crates/app/src/overview.rs` (pure functions
`build_overview`, `now_doing`, `format_elapsed`, `subagent_summary`, `pick_banner`), the app-side detail module
`crates/app/src/overview_detail.rs` (`build_detail`), the renderer modules `renderer::overview_model` (the D20 model types,
`OverviewHit`, `DialogSubject`), `renderer::overview_layout` (`layout_overview`,
`layout_detail`, `compose`) and `renderer::overview_panel` (the
`DrawLayer`), `prompt::blocking_subject`, `SessionTelemetry::with_digest`, and
`flow_telemetry`'s `SessionDigest`.

### R0: The Cmd+Shift+F overlay shows the Agent Monitor overview and detail instead of the graph — D1, D16

#### R0.1: Opening shows the overview, not the graph (manual)
- **Given**: the app runs with a Claude session in one tab and a Codex (or other) agent in another
- **When**: the user presses `Cmd+Shift+F`
- **Then**: the window shows the `Agent Monitor` header, four tiles, and one row per session
  titled by project name and agent kind; no project card, curved edge or `N finished` chip is
  drawn

#### R0.2: Detail round trip (manual)
- **Given**: the overview is open with a Claude session that ran subagents
- **When**: the user clicks that row, then presses `Esc`, then presses `Esc` again
- **Then**: the first `Esc` returns from the session detail (timeline bars from the log) to
  the overview; the second closes the overlay

### R1: Status buckets, counts and order — D2, D14

#### R1.1: CardStatus and the two flags map to a bucket
- **Given**: sessions whose resolved `CardStatus` is (a) `WaitingForInput` with
  `blocking_dialog = true`, (b) `WaitingForInput` with `graph_asks_user = true`, (c)
  `WaitingForInput` with neither, (d) `Working`, (e) `Ready`, (f) `Hidden`
- **When**: `overview::build_overview` builds the model
- **Then**: their buckets are `NeedsYou`, `NeedsYou`, `Idle`, `Working`, `Idle`, `Agent`, with
  pill labels `Needs you`, `Needs you`, `Idle`, `Working`, `Idle`, `Agent`

#### R1.5: A finished Claude turn is Idle, not Needs you
- **Given**: a Claude tab whose log ends with an assistant `end_turn` and whose screen shows no
  dialog (its sidebar card reads `Waiting for input`)
- **When**: `build_overview` builds the model
- **Then**: the row's bucket is `Idle`, the Needs-you count is 0 and no banner is built

#### R1.6: The worker publishes `graph_asks_user` only on graph passes
- **Given**: an Antigravity tab whose transcript's last step is an `ask_question` call
- **When**: the tabmeta worker runs a pass with the graph wanted, then one with it not wanted
- **Then**: `TabMeta.graph_asks_user` is true after the first and false after the second, and
  `TabMeta.graph_agent_cwd` is the agent process's cwd after the first and `None` after the
  second

#### R1.7: A non-active pane asking the user is Needs you
- **Given**: a split tab whose non-active pane runs Antigravity with `PaneMeta.agent` state
  `Waiting` and `PaneMeta.graph_asks_user = true`, and a second non-active pane with state
  `Waiting` and the flag false
- **When**: `build_overview` builds the model
- **Then**: the first pane's row is `NeedsYou`, the second's `Idle`

#### R1.2: Rows sort by bucket, then session order
- **Given**: sessions in strip order Idle(tab 1), NeedsYou(tab 2), Working(tab 3),
  Working(tab 4), Agent(tab 5)
- **When**: `build_overview` builds the model
- **Then**: rows are ordered tab 2, tab 3, tab 4, tab 1, tab 5

#### R1.3: Counts
- **Given**: the five sessions of R1.2
- **When**: `build_overview` builds the model
- **Then**: counts are needs-you 1, working 2, idle 1, all 5; the header reads `5 sessions`;
  needs-you + working + idle + agent-row count equals 5

#### R1.4: Only agent sessions are rows
- **Given**: three tabs, one plain zsh, one Claude, one split into two tmux panes each running
  an agent
- **When**: `build_overview` builds the model
- **Then**: it has exactly three rows (Claude tab, two pane sessions); the zsh tab has none;
  a non-active pane's status is `resolve_status(pane.agent, false, false, None)`'s bucket

### R2: Row identity — D3

#### R2.1: Title is project name and agent kind
- **Given**: a Claude session whose `TabMeta.repo.main_root` is `/u/IdeaProjects/terminal`
  and whose `task` is `Base directory for this skill: …`
- **When**: `build_overview` builds the model
- **Then**: the row's project is `terminal`, its agent label `Claude Code`, and the task text
  appears only as the row's prompt line `“Base directory for this skill: …”`

#### R2.2: Agent labels for every kind
- **Given**: sessions of kind Claude, Codex, Antigravity, Gemini, Aider
- **When**: `build_overview` builds the model
- **Then**: labels are `Claude Code`, `Codex`, `Antigravity`, `Gemini`, `Aider` with colours
  `#E9967A`, `#A9B8FF`, `#86D1C4`, `#B8B1A7`, `#B8B1A7`

#### R2.3: Linked worktree shows the worktree directory
- **Given**: a session with `repo.linked = true`, branch `fix/mouse-drag-not-working`, agent
  cwd `/u/IdeaProjects/terminal-fix-mouse-drag-not-working`
- **When**: `build_overview` builds the model
- **Then**: line 2 reads `fix/mouse-drag-not-working · terminal-fix-mouse-drag-not-working`;
  a main-worktree session's line 2 is the branch alone

#### R2.4: No repository, no branch, no prompt
- **Given**: a post-ready session with `repo = None`, no branch, cwd `/u/scratch/notes`, no task
- **When**: `build_overview` builds the model
- **Then**: project is `notes`, line 2 is the `~`-abbreviated cwd, and the row has no prompt line

#### R2.5: Resolving draws no rows
- **Given**: the overlay is open and the worker has not published `graph_ready` for this epoch
- **When**: the overlay rebuilds
- **Then**: the model is `resolving`: the header reads `resolving…` and no row exists

### R3: Now doing and elapsed — D4, D7, D11

#### R3.1: Needs you with and without a subject
- **Given**: Needs-you sessions whose rows have `blocking_subject` =
  `Some(Command("ls -d ~/.config"))`, `Some(Question)`, `Some(Plan)`, and `None`
- **When**: `overview::now_doing` formats them
- **Then**: the texts are `Approval needed · ls -d ~/.config`, `Question waiting`,
  `Plan approval waiting`, and `Waiting for your input`

#### R3.2: Working on a tool
- **Given**: a Working Claude session whose digest `last_tool` is `("Bash", Some("git branch -d
  feat/x"), _)` and which has no running subagent
- **When**: `now_doing` formats it
- **Then**: the text is `Bash · git branch -d feat/x`; with summary `None` it is `Bash`; with no
  `last_tool` it is `Working`

#### R3.3: Waiting on subagents
- **Given**: a Working session with 2 running subagents, the newest started at t=1000, and
  `last_work_tool.start_ms` = 1000 (then = 900, then absent, then = 1001)
- **When**: `now_doing` formats it
- **Then**: the text is `Waiting on 2 subagents` for 1000, 900 and absent, and the
  `{Tool} · {summary}` form for 1001; with one running subagent it reads `Waiting on 1 subagent`

#### R3.4: Idle, unknown and non-Claude
- **Given**: an Idle session, an `Agent`-bucket session, and a Working Codex session with no digest
- **When**: `now_doing` formats them
- **Then**: `Turn finished · ready for next prompt`, `Agent running`, `Working`

#### R3.5: Elapsed format
- **Given**: `last_prompt_ms` 30 s, 12 min, and 1 h 41 min before `now_ms`, and a session with no digest
- **When**: `overview::format_elapsed` formats them
- **Then**: `<1m`, `12m`, `1h 41m`, and `—`

#### R3.6: Tool summary in telemetry
- **Given**: a Claude log with finished `Bash {command: "git status\n--short"}`, `Edit
  {file_path: "/a/b/lib.rs"}`, `Grep {pattern: "fn main"}`, `WebFetch {url:
  "https://docs.rs/x"}`, a 200-char `Bash` command and an unknown `Foo` tool, folded by a
  `SessionTelemetry::with_digest` instance
- **When**: `snapshot()` is taken after each
- **Then**: `last_tool` summaries are `git status`, `lib.rs`, `fn main`, `docs.rs`, the first 79
  chars plus `…`, and `None`; an `Agent` call updates `last_tool` but not `last_work_tool`

#### R3.7: The ai-monitor fold does no digest work
- **Given**: the same log folded by a `SessionTelemetry` built without the digest flag
- **When**: `snapshot()` is taken
- **Then**: `last_tool`, `last_work_tool`, `last_prompt_ms` are `None`, `turns` and `events` are
  empty, every `report_preview` is `None`, and every pre-existing snapshot field equals the
  digest-flag fold's (`subagents` compared with `report_preview` cleared)

### R4: Tiles, banner and filter — D5, D6

#### R4.1: Banner picks the first waiting row
- **Given**: sorted rows with two Needs-you sessions, Claude `receipt-tracker` first
  (subject `./gradlew test`)
- **When**: `overview::pick_banner` runs
- **Then**: it returns target `(tab, pane)` of the Claude row and text
  `Claude Code · receipt-tracker — Approval needed · ./gradlew test`, suffix ` (+1 more)`; with no
  Needs-you row it returns `None`

#### R4.2: Banner ignores the filter
- **Given**: filter `Idle` and one Needs-you session
- **When**: `build_overview` builds the model
- **Then**: the banner is present and the table holds only Idle rows

#### R4.3: Filter restricts rows, not tiles
- **Given**: the five sessions of R1.2 and filter `Working`
- **When**: `build_overview` builds the model
- **Then**: rows are tab 3 and tab 4; tile counts are still 1/2/1; filter labels read
  `All 5`, `Needs you 1`, `Working 2`, `Idle 1`

#### R4.4: Empty filter
- **Given**: no Idle session and filter `Idle`
- **When**: the renderer lays out the model through `overview_layout::layout_overview`
- **Then**: the table contains the text `No sessions in this filter.`

#### R4.5: Filter press, persistence and scroll reset
- **Given**: the overview is open, scrolled down 3 lines, filter `All`
- **When**: a press lands on the `Working` filter tab (`OverviewHit::Filter`), then the overlay
  is closed and reopened via the `Overlay` `on_key` chord
- **Then**: the filter is `Working` with table scroll 0, and after reopening is still `Working`;
  a fresh `FlowOverlay` starts at `All`

#### R4.6: Subagent tile totals
- **Given**: sessions with subagents: 2 running + 1 done; 7 done; 1 failed
- **When**: `build_overview` builds the model
- **Then**: the tile reads running 2, done 9 (failed counted in done), and the bar fractions are
  running 2/11, done 8/11, failed 1/11

### R5: Subagent squares and tree — D8

#### R5.1: Square colours and summary
- **Given**: a session whose subagents in log order are running, done (`ended_ok`), failed
  (`completed && !ended_ok`)
- **When**: `overview::subagent_summary` runs
- **Then**: squares are `#E8744F`, `#6FA67F`, `#E5534B` in that order and the summary is
  `1 running · 2 done`; with no running one the summary is `2 done`; with none the cell is `—`

#### R5.2: Overflow to `+k`
- **Given**: a session with 32 subagents and a Subagents column of 220 logical px (two lines of
  14 squares)
- **When**: `layout_overview` lays the row out
- **Then**: 27 squares are drawn and the 28th slot reads `+5`

#### R5.3: Expand toggles the tree
- **Given**: a collapsed row `(tab 3, pane 0)` with 3 subagents
- **When**: a press lands on its chevron (`OverviewHit::Expand(3, 0)`)
- **Then**: `(3, 0)` is in the expanded set, the model's row has 3 tree lines with type,
  description, status word and duration; a second press removes it; a row with no subagents has
  no chevron hit

#### R5.4: Tree durations
- **Given**: a finished subagent of 171 s, a running one started 4 min 10 s ago, and one with no
  `start_ms`
- **When**: `build_overview` formats the tree lines
- **Then**: durations read `2m 51s`, `4m`, `—`; type falls back to `agent` and description to `—`

#### R5.5: Expanded set survives reopen and prunes
- **Given**: `(3, 0)` expanded, overlay closed and reopened, then tab 3 closed
- **When**: the overlay rebuilds after each
- **Then**: after reopen `(3, 0)` is still expanded; after tab 3 closes the set no longer holds it

### R6: The dialog subject — D9

#### R6.1: Claude permission dialog
- **Given**: the `REAL_DIALOG` fixture
- **When**: `prompt::blocking_subject` reads it
- **Then**: it returns `Some(DialogSubject::Command("ls -d ~/.config"))`

#### R6.2: Antigravity dialog
- **Given**: the `REAL_ANTIGRAVITY_DIALOG` fixture
- **When**: `blocking_subject` reads it
- **Then**: it returns `Some(DialogSubject::Command("ls -la"))`

#### R6.3: Question and plan dialogs, no match
- **Given**: the `REAL_ASK_MULTI` and `REAL_PLAN_WIDE` fixtures and a plain zsh screen
- **When**: `blocking_subject` reads them
- **Then**: `Some(Question)`, `Some(Plan)`, `None`

#### R6.4: Subject published with the level
- **Given**: a session whose screen shows `REAL_DIALOG`, then a second dialog with a different
  command, then no dialog
- **When**: `Session::rescan_dialog` runs after each screen and the tab row is synced
- **Then**: `SidebarRow.blocking_subject` reads `ls -d ~/.config`, then the new command (and the
  rescan reports a change although the level stayed true), then `None` with
  `blocking_dialog = false`

#### R6.5: Long subjects truncate
- **Given**: a dialog whose command row is 150 characters
- **When**: `blocking_subject` reads it
- **Then**: the subject is a `Command` of 80 characters, the last being `…`

### R7: Navigation and hits — D10, D20, D22

#### R7.1: Row press opens detail
- **Given**: the overview with row `(tab 4, pane 0)`
- **When**: `FlowOverlay::on_press` receives a point inside the row but outside both buttons
- **Then**: the view is `Detail` for `(4, 0)`; no tab switch is requested

#### R7.2: Open, banner and focus switch tabs
- **Given**: the overview (or detail) for a session in tab 4
- **When**: a press lands on its `Open` button, the banner's `Open session →`, or the detail's
  `Focus terminal`
- **Then**: the app is asked to activate tab 4 and the overlay closes

#### R7.3: Keys by view
- **Given**: the detail view is open
- **When**: `on_key` receives `Esc`; then, from the detail again, `Cmd+Shift+F`; then, in the
  overview, `Esc`
- **Then**: `Esc` returns to the overview; `Cmd+Shift+F` closes the overlay; `Esc` in the
  overview closes it; every other key is swallowed

#### R7.4: Back button
- **Given**: the detail view
- **When**: a press lands on `‹ All sessions` (`OverviewHit::Back`)
- **Then**: the view is the overview with its prior filter, expanded set and scroll

#### R7.5: Vanished session
- **Given**: the detail for tab 4 is open, then tab 4 closes
- **When**: the overlay rebuilds
- **Then**: the view is the overview

#### R7.6: Stale press
- **Given**: hits published for tab 4, then tab 4 closed before a rebuild
- **When**: a press lands on tab 4's `Open` button
- **Then**: no tab switch is requested and the overlay stays open

#### R7.7: Reopen resets view and scroll
- **Given**: the detail view open with the lanes scrolled
- **When**: the overlay is closed and reopened
- **Then**: the view is the overview and every scroll offset is 0

#### R7.8: Copy resume command
- **Given**: the detail of a Claude session with `session_id` `abc-123`, and of a Codex session
- **When**: a press lands on `Copy resume cmd` (`OverviewHit::CopyResume`) through
  `FlowOverlay::on_press`
- **Then**: the overlay pushes `AppRequest::CopyToClipboard("claude --resume abc-123")` into
  the `AppCtx`; the Codex detail publishes no `CopyResume` hit

### R8: Detail timeline, panel and events — D12, D13, D19

#### R8.1: Turns end only when background work is done
- **Given**: a Claude log: human prompt at 10:00, background `Agent` launched at 10:01, assistant
  `end_turn` at 10:02, the agent's task notification at 10:09, `end_turn` at 10:10, folded with
  the digest flag
- **When**: `snapshot()` is taken
- **Then**: `turns` holds one turn `[10:00, 10:10]`; before the notification line is folded its
  `end_ms` is `None`

#### R8.2: Main lane segments
- **Given**: the turn of R8.1, a subagent running 10:01–10:09, now 10:20
- **When**: `overview_detail::build_detail` builds the lanes
- **Then**: the main lane is working 10:00–10:01, waiting on subagent 10:01–10:09, working
  10:09–10:10, idle 10:10–10:20, as window fractions

#### R8.3: Window and ticks
- **Given**: `tz_offset_s = 0`; a session whose first event is 3 h ago, now 14:37
- **When**: `build_detail` builds the timeline
- **Then**: the window is 13:37–14:37, ticks are at `13:40`, `13:50`, `14:00`, `14:10`, `14:20`,
  `14:30`; a subagent 13:20–13:45 is clipped to start at fraction 0; one ending 13:30 has no lane

#### R8.4: Short sessions start at their first event
- **Given**: a session whose first event is 20 min ago
- **When**: `build_detail` builds the timeline
- **Then**: the window starts at that first event

#### R8.5: Selection and panel
- **Given**: a detail with 7 lanes and no selection
- **When**: `build_detail` runs, then a press lands on lane 2 (`OverviewHit::Lane(2)`)
- **Then**: lane 6 (the newest) is selected first, then lane 2; the panel shows its description,
  type, status, `Started` as local `HH:MM:SS`, duration, and up to four report lines or `—`

#### R8.6: Report preview from both result forms
- **Given**: a synchronous `Agent` result whose text has 6 non-blank lines, and a background
  agent whose task notification carries a `<result>` of 2 lines, folded with the digest flag
- **When**: `snapshot()` is taken
- **Then**: the first subagent's `report_preview` is its first 4 non-blank lines (≤ 400 chars),
  the second's its 2 lines

#### R8.7: Recent events
- **Given**: a window holding 12 events: tool calls, task starts, task done, one failed task, two
  turn ends
- **When**: `build_detail` builds the list
- **Then**: it holds the newest 8, newest first, each `HH:MM` + `Turn finished` /
  `Task start · {d}` / `Task done · {d}` / `Task failed · {d}` / `{Tool} · {summary}`

#### R8.8: Non-Claude detail
- **Given**: an Antigravity session with no digest
- **When**: `build_detail` builds it
- **Then**: it has the header data and no lanes, and the layout shows `No timeline — this agent's log is not read.`

#### R8.9: The digest reaches the overlay
- **Given**: the overlay open on a Claude tab whose log gains a subagent launch line
- **When**: the flow telemetry thread's next pass publishes
- **Then**: the slot holds that log's `SessionDigest` whose `subagents` include the new one, and
  the generation counter has advanced

### R9: Layout and drawing — D15, D21

#### R9.1: Column widths at full width
- **Given**: content width W = 1376 at scale 1
- **When**: `layout_overview` lays out the table
- **Then**: Status 120, Subagents 220, Elapsed 88, buttons 104, gaps 16, and Session : Now doing
  share the rest 1.2 : 2

#### R9.2: Narrow thresholds
- **Given**: W = 1100, 1000, 900, 750, 700
- **When**: `layout_overview` lays out the table
- **Then**: 1100: Now doing ≥ 160; 1000: Subagents 88 with no summary text and tiles 2 × 2; 900:
  no Elapsed column; 750: no Subagents column; 700: only the header and `Window too narrow —
  widen it to see sessions.`; in every case no two text runs' rects overlap

#### R9.3: Detail stacks when narrow
- **Given**: W = 1376, then 900
- **When**: `layout_detail` lays out the detail
- **Then**: 1376: timeline and a 340 px side panel side by side; 900: side panel below the timeline

#### R9.4: Whole-line scrolling
- **Given**: 30 rows, one expanded with 5 tree lines, in a table area fitting 8 lines
- **When**: wheel notches move the table scroll
- **Then**: the offset is a line index clamped to `[0, total − visible]`; header, tiles, banner
  and filter keep their rects; no line is drawn partially

#### R9.5: The frame shows the design's colours
- **Given**: a GPU and a model with one Needs-you row and a row with one running and one done
  subagent
- **When**: the overview layer draws through `Renderer::capture`
- **Then**: the pixel 8 physical px right of the Needs-you tile's left edge at its vertical
  middle is `#30271B` (the tile fill, D21) within ±3 per channel; the running square's centre
  is `#E8744F` and the done square's `#6FA67F`, each within ±3 per channel; the test is skipped
  only when no GPU adapter exists

#### R9.6: Display face for tile numbers
- **Given**: the overview layer's first visible frame at scale 2
- **When**: its atlases are ensured
- **Then**: the layer's `atlas_ppems()` lists `OverviewFace::Display` at 60 physical px beside
  `Title` and `Meta`, and the laid-out tile-number runs carry the Display face

### R10: Freshness, cost and degradation — D17, D18, D23

#### R10.1: Minute rebuild
- **Given**: an open overview with nothing else changing
- **When**: `now_ms` crosses a minute boundary
- **Then**: `FlowOverlay::rebuilds()` advances by exactly 1; further frames within the same
  minute leave it unchanged

#### R10.2: Closed overlay costs nothing new
- **Given**: the overlay closed and the ai-monitor open
- **When**: logs grow
- **Then**: `FlowOverlay::telemetry_running()` is false,
  `FlowOverlay::rebuilds()` does not advance, and a `SessionTelemetry::new()` (the
  ai-monitor's construction) folding the same lines has empty digest fields (R3.7)

#### R10.3: Missing data reads `—`
- **Given**: a Claude session with no resolvable log
- **When**: `build_overview` builds it
- **Then**: now-doing is the state-only text, elapsed `—`, subagents `—`

#### R10.4: No sessions
- **Given**: a ready snapshot with no agent session
- **When**: the overview is laid out
- **Then**: tiles read 0 and the table shows `No agent sessions in this window.`

#### R10.5: Unchanged frame writes nothing
- **Given**: an open overview whose model, scroll, hover and view are unchanged
- **When**: a second frame is recorded
- **Then**: the layer's `compose_count()` does not advance and no buffer is written

### R11: Amendment fixes — D24, D25, D26, D27

#### R11.1: The tab label skips non-human user records
- **Given**: a Claude log whose newest user records are, newest first, a `task-notification`
  (`origin.kind: "task-notification"`), a `peer` message, an `isMeta` skill injection
  (`Base directory for this skill: …`), and then a human prompt `fix the duplicate G7 numbering`
- **When**: `ai_monitor::activity::recent_prompts` reads it
- **Then**: the first entry is `fix the duplicate G7 numbering`; none of the three others appears

#### R11.2: The prompt is drawn with one pair of quotes
- **Given**: a row whose model prompt is `“fix the duplicate G7 numbering”`, and one whose prompt
  is 300 characters inside `“…”`
- **When**: `renderer::overview_layout::layout_overview` lays them out
- **Then**: the first row's prompt text run is exactly `“fix the duplicate G7 numbering”`; the
  second starts with exactly one `“`, ends with `…”`, and contains no `““`

#### R11.3: An Antigravity transcript yields a digest
- **Given**: an Antigravity transcript with a `USER_INPUT` step at t1, an `invoke_subagent` call
  whose subagent completes, and a tool-less planner response at t2 with nothing pending, folded by
  `SessionTelemetry::with_format(LogFormat::Antigravity).with_digest()`
- **When**: `snapshot()` is taken
- **Then**: `subagents` holds the subagent as completed, `last_prompt_ms` is t1, and `turns` holds
  one turn `[t1, t2]`

#### R11.4: The worker publishes the Antigravity transcript on graph passes only
- **Given**: an Antigravity tab whose `agy` holds a presence lock naming a transcript
- **When**: the tabmeta worker runs a graph pass, then a non-graph pass
- **Then**: `TabMeta.graph_transcript` is that transcript's path after the first and `None` after
  the second; a pane running `agy` gets its own `PaneMeta.graph_transcript` (in-crate unit test,
  as R1.6)

#### R11.5: An Antigravity row shows its subagents and elapsed
- **Given**: an Antigravity session whose `graph_transcript` maps to a digest with 2 subagents
  (one running, one done) and `last_prompt_ms` 3 minutes before `now_ms`
- **When**: `app::overview::build_overview` builds the model
- **Then**: the row has 2 squares, summary `1 running · 1 done`, and elapsed `3m`

#### R11.6: An Antigravity detail has a timeline
- **Given**: the R11.5 row and digest
- **When**: `app::overview_detail::build_detail` builds it
- **Then**: `has_timeline` is true, there are 2 lanes, and `resume_cmd` is `None`; a Codex row
  with no digest still has `has_timeline` false

#### R11.7: A handed-back notification does not hold the card on Working (D27)
- **Given**: a Claude log ending, oldest first, in a background `Agent` launch, a `peer` hand-back
  (`origin.handback: true`, `origin.from: a3ee`) answered by an `end_turn`, and a
  `task-notification` user record for `a3ee`; and the same log without the hand-back
- **When**: `ai_monitor::activity::tail_state` reads each
- **Then**: the first's `last` is `AssistantEndTurn`; the second's is `UserPrompt`; a hand-back
  naming another task leaves `UserPrompt`

## Tasks

Test-first: every `TTn` writes **new** files only, named `tt{n}_overview_*.rs` (or
`tt{n}_digest_*.rs` / `tt{n}_dialog_*.rs`) under the crate's `tests/` directory, which
`cargo test --workspace` collects. Basenames must not collide with the existing
`tt{n}_graph_*.rs` files. A TT may fail to compile until the implementation task that
depends on it lands; that is its red state. Functions a TT calls from `tests/` are `pub`
(the app crate is a library — architecture "app" boundary). Infra tasks: T1, T4.

### T1: Session digest in ai-monitor telemetry, behind an opt-in flag [infra]
- **Fulfills**: R3 (R3.6, R3.7), R8 (R8.1, R8.6)
- **Depends on**: (none)
- **Files**: `crates/ai-monitor/src/telemetry.rs` (and `crates/ai-monitor/src/activity.rs` only
  to share the G8 background start/completion and human-prompt predicates, not to change them)
- **Work**: `SessionTelemetry::with_digest(self) -> Self` builder flag (D19); when set, `apply`
  also records per finished tool a one-line summary (D11's table), `last_tool`,
  `last_work_tool`, `last_prompt_ms`, `turns` (D19's end rule, newest 64), `events` (newest 64:
  turn end, task start, task done/failed, finished tool), and `Subagent.report_preview` from
  the sync `Agent` result text or the task notification's `<result>`. Without the flag every new
  field stays empty and no extra work is done. Existing snapshot fields unchanged in both modes.

### T2: Dialog subject from the screen to the sidebar row [vertical]
- **Fulfills**: R6
- **Depends on**: T4, TT6  (T4 first: both edit `crates/app/src/lib.rs`)
- **Files**: `crates/app/src/prompt.rs`, `crates/app/src/session.rs`, `crates/app/src/tabs.rs`,
  `crates/renderer/src/sidebar_row.rs` (the `blocking_subject` field),
  `crates/app/src/lib.rs` (`pub use prompt::blocking_subject`), `crates/app/src/terminal_app.rs`
  (`patch_dialog_rows` carries the subject to the row setter)
- **Work**: `pub fn blocking_subject(screen: &str) -> Option<DialogSubject>` (the enum is T4's),
  re-exported for tests; computed in
  `Session::rescan_dialog` only on the scanning branch that matched, held on the held branch
  (D9); `SidebarRow.blocking_subject` set beside
  `blocking_dialog` with a subject change reported as a change (D9, D18).

### T3: Graph-pass fields — `graph_asks_user` and `graph_agent_cwd`, per tab and per pane [vertical]
- **Fulfills**: R1 (R1.6), R2 (R2.3, R2.4 — the cwd they read)
- **Depends on**: TT1, TT2
- **Files**: `crates/app/src/tabmeta/mod.rs`, `crates/app/src/tabmeta/worker.rs`,
  `crates/app/src/tabmeta/activity.rs`
- **Work**: on graph passes only, publish on `TabMeta` and on each `PaneMeta`:
  `graph_asks_user` = that session's resolved log's last meaningful message is
  `LastMessage::AssistantAsksUser` (D2), and `graph_agent_cwd` = the detected agent process's
  cwd (the one already read for `repo`, D3/D10). `false` / `None` otherwise and on non-graph
  passes.

### T4: Contracts — renderer model types, app policy signatures, digest slot [infra]
- **Fulfills**: R8 (R8.9)
- **Depends on**: T1
- **Files**: new `crates/renderer/src/overview_model.rs` (`OverviewModel`, `SessionRowData`,
  `Bucket`, `Filter`, `SubagentCell`, `TreeLine`, `DetailData`, `Lane`, `Segment`, `EventLine`,
  `View`, `OverviewHit` — D20 — and `DialogSubject { Command(String), Question, Plan }`, D9:
  renderer-side because `SidebarRow` carries it and the renderer may not import app types), `crates/renderer/src/lib.rs` (module export); new
  `crates/app/src/overview.rs` and `crates/app/src/overview_detail.rs` with the public
  signatures named in `## Requirements` returning empty values; `crates/app/src/lib.rs`;
  `crates/app/src/flow_telemetry.rs` (slot becomes `HashMap<PathBuf, SessionDigest>`, built with
  `with_digest`); `crates/app/src/flow.rs` only as far as the slot type change requires (`build_graph` reads
  `digest.subagents`); `crates/app/tests/tt7_flow_telemetry.rs` re-pointed to the digest slot.
- **Signatures** (app; `SessionDigest` lives in `flow_telemetry`, `ExpandSet` is the old
  `FoldSet`, `HashSet<(TabId, u32)>`):
  - `pub fn build_overview(rows: &[SidebarRow], metas: &HashMap<TabId, TabMeta>, home:
    Option<&Path>, digests: &HashMap<PathBuf, SessionDigest>, expanded: &ExpandSet, filter:
    Filter, active: usize, now_ms: u64) -> (OverviewModel, ExpandSet)`
  - `pub fn now_doing(row: &NowDoingInput<'_>) -> String`, where `NowDoingInput { bucket:
    Bucket, subject: Option<&DialogSubject>, digest: Option<&SessionDigest> }`
  - `pub fn format_elapsed(last_prompt_ms: Option<u64>, now_ms: u64) -> String`
  - `pub fn subagent_summary(subagents: &[Subagent], now_ms: u64) -> (Vec<SubagentCell>,
    String)`
  - `pub fn pick_banner(rows: &[SessionRowData]) -> Option<Banner>` (`Banner { tab: u64, pane:
    u32, text: String, more: usize }` in `overview_model`)
  - `pub fn build_detail(row: &SessionRowData, digest: Option<&SessionDigest>, selected:
    Option<usize>, now_ms: u64, tz_offset_s: i32) -> DetailData`
- **Work**: compile-ready contracts so TTs can be written against them; R8.9's publication is
  real here (the thread publishes the digest; the old graph keeps reading its subagents).

### T5: Overview policy — buckets, identity, now-doing, elapsed, tiles, banner, filter, squares [vertical]
- **Fulfills**: R1 (R1.1–R1.5, R1.7), R2, R3 (R3.1–R3.5), R4 (R4.1–R4.3, R4.6), R5 (R5.1, R5.4), R10 (R10.3)
- **Depends on**: T2, T3, T4, TT1, TT2, TT3, TT4, TT5, TT10
- **Files**: `crates/app/src/overview.rs` (+ unit tests in it)
- **Work**: D2–D8, D14, D17 as pure functions over `rows`, `metas`, `home`, digests, expanded
  set, filter, active index, `now_ms`.

### T6: Detail policy — window, ticks, lanes, segments, selection, events [vertical]
- **Fulfills**: R8 (R8.2–R8.5, R8.7, R8.8)
- **Depends on**: T4, TT8  ← parallel with T5 (separate file)
- **Files**: `crates/app/src/overview_detail.rs`
- **Work**: D12, D13 as T4's `build_detail` signature,
  producing `DetailData` with window fractions and `HH:MM` / `HH:MM:SS` labels computed as
  `(ms / 1000 + tz_offset_s)` wall-clock. The offset is the local UTC offset in seconds,
  read by `FlowOverlay` (T8) once per rebuild through `libc::localtime_r(...).tm_gmtoff`;
  tests pass a fixed offset, so every time in R8's GWTs is wall-clock at offset 0.

### T7: Renderer — overview and detail layout, palette, Display face, the draw layer [vertical]
- **Fulfills**: R4 (R4.4), R5 (R5.2), R8 (R8.8 layout text), R9, R10 (R10.4, R10.5)
- **Depends on**: T4, TT4, TT5, TT8, TT9, TT10  ← parallel with T5 and T6
- **Files**: new `crates/renderer/src/overview_layout.rs` (`layout_overview`, `layout_detail`,
  `compose`), new `crates/renderer/src/overview_panel.rs` (the `DrawLayer` at z 110, two-stage
  cache, own atlases incl. the 30 px Display face, D21), new `crates/renderer/src/overview_theme.rs`
  (D21 palette), `crates/renderer/src/lib.rs`
  (new module exports only). T7 adds modules and deletes nothing: `graph_layout.rs` and
  `graph_panel.rs` stay compiling and in use until T8 switches the app over.
- **Work**: D15's widths and thresholds, whole-line scrolling, D8's square wrapping and `+k`,
  hits, and public observers on the layer patterned on `graph_panel.rs`'s `compose_count()`
  / `atlas_builds()`: `compose_count() -> u64` (R10.5) and `atlas_ppems() -> Vec<(OverviewFace, f32)>`
  (R9.6). The Display face is **not** a `CardFace` variant: `overview_panel` owns
  `OverviewFace { Title, Meta, Display }` and its own three atlases (Title and Meta at
  `CardFace`'s ppems, Display at 30 logical px), so the sidebar's `CardAtlases`, its exhaustive
  `match run.face` (`sidebar_row.rs:1133`) and its atlas cost are untouched.

### T8: Overlay wiring — views, keys, presses, filter, expand, minute key, copy [vertical]
- **Fulfills**: R0, R4 (R4.5), R5 (R5.3, R5.5), R7, R10 (R10.1, R10.2)
- **Depends on**: T5, T6, T7, TT4, TT5, TT7, TT10
- **Files**: `crates/app/src/flow.rs` (FlowOverlay drives `build_overview` / `build_detail`,
  view state, D10 keys and hits, D18 rebuild key with the minute term, `OverviewHit` dispatch,
  `CopyResume` as an `AppRequest::CopyToClipboard`), `crates/app/src/lib.rs` (`feature_overlays`,
  line ~215) if the layer type changes; delete `build_graph` and the app graph
  tests covering only removed behaviour (`tt5_graph_shape.rs`, `tt7_graph_header.rs`,
  `tt8_graph_subagents.rs`, `tt9_graph_click.rs`, `tt11_graph_wheel.rs`); keep and re-point
  `tt1_graph_keys.rs`, `tt2_graph_pointer.rs`, `tt3_graph_monitor.rs`, `tt4_graph_repo.rs`,
  `tt6_graph_status.rs`, `tt6_tmux_panes.rs`, `tt7_flow_telemetry.rs`, `tt14_graph_agents.rs`;
  `crates/app/src/terminal_app.rs` (its `graph_panel` reference, ~line 3648, re-pointed) and
  `crates/app/src/lib.rs` (~line 243); `crates/app/src/seam.rs` (new
  `AppRequest::CopyToClipboard(String)`, handled in `terminal_app.rs` through `copy.rs`);
  then delete `crates/renderer/src/graph_layout.rs`, `graph_panel.rs`, their exports in
  `crates/renderer/src/lib.rs`, and the renderer graph tests covering only removed behaviour
  (`tt9_graph_press.rs`, `tt10_graph_layout.rs`, `tt11_graph_scroll.rs`,
  `tt12_graph_frame.rs`; `tt3_graph_inset.rs` is kept and re-pointed to the new layer).
  Adds public observers on `FlowOverlay`: `rebuilds() -> u64` (R10.1, R10.2) and
  `telemetry_running() -> bool` (R10.2). Reads the local UTC offset once per rebuild with
  `libc::localtime_r(...).tm_gmtoff` and passes it to `build_detail` as `tz_offset_s`.
- **Work**: R0 checked by hand with `/run-term`.

### T9: Docs — vocabulary and gaps [vertical]
- **Fulfills**: R0
- **Depends on**: T8
- **Files**: `docs/domain.md` (replace "session graph", "project card", "slot" entries with the
  overview, detail, bucket and digest vocabulary), `docs/architecture.md` (G10 rewritten for the
  overview; G8 gains D9's two dialog strings and the `end_turn` ≠ Needs-you note)

### TT1: acceptance tests for R1's GWTs
- **Fulfills**: R1 (tests)
- **Depends on**: T1, T4

### TT2: acceptance tests for R2's GWTs
- **Fulfills**: R2 (tests)
- **Depends on**: T1, T4

### TT3: acceptance tests for R3's GWTs
- **Fulfills**: R3 (tests)
- **Depends on**: T1, T4

### TT4: acceptance tests for R4's GWTs
- **Fulfills**: R4 (tests)
- **Depends on**: T1, T4

### TT5: acceptance tests for R5's GWTs
- **Fulfills**: R5 (tests)
- **Depends on**: T1, T4

### TT6: acceptance tests for R6's GWTs
- **Fulfills**: R6 (tests)
- **Note**: the `prompt.rs` fixtures are `#[cfg(test)] pub(crate)` and unreachable from
  `tests/`; TT6 copies `REAL_DIALOG`, `REAL_ANTIGRAVITY_DIALOG`, `REAL_ASK_MULTI` and
  `REAL_PLAN_WIDE` verbatim into its own file and calls `app::blocking_subject`
- **Depends on**: T1, T4

### TT7: acceptance tests for R7's GWTs
- **Fulfills**: R7 (tests)
- **Depends on**: T1, T4

### TT8: acceptance tests for R8's GWTs
- **Fulfills**: R8 (tests)
- **Depends on**: T1, T4

### TT9: acceptance tests for R9's GWTs
- **Fulfills**: R9 (tests)
- **Depends on**: T1, T4

### TT10: acceptance tests for R10's GWTs
- **Fulfills**: R10 (tests)
- **Depends on**: T1, T4

## External Dependencies

### Pre-work
(none)

### Post-work
- Eyeball R0.1/R0.2 in the running app (`/run-term`): a Claude permission dialog in one tab,
  another agent in a second, `Cmd+Shift+F`, the banner, a row's detail, `Esc` twice.
- `cargo mutants -f crates/app/src/overview.rs -j 4` and for `overview_detail.rs`,
  `overview_layout.rs`, `prompt.rs`: record survivor counts (D23).
- The compact sidebar board (D16) needs the person's decision on its home before a follow-up
  spec.

## Spec Review
- **Verdict**: NEEDS_FIX
- **Reviews**: 3 + fix confirmation
- **Fixed**:
  - [Task order] T7/T8 — graph module deletions and the `terminal_app.rs` / `lib.rs` re-pointing moved into T8, so the workspace compiles between them
  - [Readiness] TT6 — fixtures copied verbatim into the test file; `app::blocking_subject` re-exported
  - [Grounding] R8 — `build_detail` placed in `overview_detail.rs` consistently
  - [Readiness] T6/T8 — `tz_offset_s: i32` from `libc::localtime_r(...).tm_gmtoff`, read by T8; R8 times at offset 0
  - [Task order] T2 — `crates/app/src/lib.rs`, `terminal_app.rs` (`patch_dialog_rows`) added to Files; T2 after T4
  - [Testability] R7.8 — asserts `AppRequest::CopyToClipboard(..)` instead of the system clipboard
  - [Testability] R10.1/R10.2/R10.5/R9.6 — `FlowOverlay::rebuilds()`, `telemetry_running()`, layer `compose_count()`, `atlas_ppems()` created by T7/T8
  - [Testability] R9.5 — exact tile fill `#30271B`, sample point, ±3 per channel
  - [Readiness] T2/D9 — subject computed only on the scanning branch, held on the held branch
  - [Task order] T4 — `tt7_flow_telemetry.rs` re-pointed in T4
  - [Testability] R3.7 — `subagents` compared with `report_preview` cleared
  - [Consistency] D19/R8.1 — a task notification resumes the turn until the next `end_turn`
  - [Readiness] T4 — full typed signatures for `build_overview`, `now_doing`, `format_elapsed`, `subagent_summary`, `pick_banner`, `build_detail`
  - [Consistency] D9 — one signature, `Option<DialogSubject>`
  - [Task order] T4/T2 — `DialogSubject` created in T4 (`overview_model.rs`)
  - [Grounding] Meta — design file moved into the spec directory
  - [Grounding] D3/R2.3/R2.4 — `graph_agent_cwd` on `TabMeta`/`PaneMeta`, published by T3
  - [Coverage] D2/T3 — `PaneMeta.graph_asks_user`, new R1.7
  - [Grounding] R9.6/T7 — `OverviewFace` owned by `overview_panel`, not a `CardFace` variant
  - [Coverage] R8.9 — Then narrowed to what T4 delivers
  - [Grounding] Requirements preamble — `OverviewHit` / `DialogSubject` in `overview_model`
- **Open**: (none — the one issue below was fixed at the user's request on 2026-10-31)
  - [fixed after review] R4.1 Then — still says "returns target `(tab, pane)` of the Codex row" while its Given and expected text name the Claude `receipt-tracker` row. One-word fix: `Codex row` → `Claude row`.
