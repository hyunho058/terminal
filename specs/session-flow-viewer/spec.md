# Spec: session-flow-viewer

## Meta
- **Created**: 2026-10-18
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-10-18 (after the unattended stop; the 3rd review's Open fixes accepted unreviewed)
- **Mode**: unattended
- **Source**: `requirements-interview-outputs/session-flow-viewer/insights.md` (6 rounds, ambiguity 0.18)
- **Superseded in part by**: `specs/session-flow-graph/spec.md` (2026-10-20) — the right-dock tree (D4 shared dock, window/tab/pane nodes, the dock-occupancy requirements) is replaced by a full-window card graph; D2, D3, D5, D9, D10, D13, D14, D15 and D18 (minus its tab-node bullet) carry over

## Goal
"session flow viewer" — a viewer that shows session flow like a tree: all sessions,
how many sessions there are, and which are processing. Requirements were settled in a
requirements interview (`requirements-interview-outputs/session-flow-viewer/insights.md`).

## Non-goals
- Claude sessions running outside this terminal (other terminals, IDEs, background jobs).
- Replacing or restyling the left sidebar or the ai-monitor panel.
- A separate OS window for the viewer.
- Showing subagents before Claude Code writes them to its log (the log omits a
  pending tool call until it returns; no screen- or process-level subagent detection).
- Per-session telemetry (tokens, cost, context usage) — that is the ai-monitor's job.
- A graphical settings surface for the viewer.

## Confirmed Goal
A key-toggled **session flow panel** that draws, for the current window, a tree
`window → tab → pane (only when the tab is split) → Claude session → subagent`,
covering only the Claude sessions running inside this terminal's tabs and panes.

- Each session node shows one of the sidebar's three states — **Working / Waiting for
  input / Ready** — and always agrees with the sidebar card for the same tab, because
  both read one source.
- A header counts sessions by state, with waiting as its own number, e.g.
  `2 working · 1 waiting / 5`.
- Subagents appear under their parent session once the log records them; finished
  subagents stay, greyed, collapsed behind an `N finished` line.
- While a subagent is running but not yet logged, the parent session reads Working and
  shows no child node.
- Clicking a node switches to that node's tab.

**Done when** the user, with the panel open, starts and finishes Claude work in two or
more tabs and watches the header's working count change live, with each node's state
matching its tab's sidebar card.

Inferences beyond the literal request:
- (inferred) The panel docks alongside the existing chrome and **reserves** terminal
  columns the way the ai-monitor does, rather than covering terminal text.
- (inferred) "Window" is the current window only; this app is single-window today, so
  the root is a single node.
- (inferred) Tabs with no Claude session are still listed (as plain, state-less nodes)
  so the tree mirrors the tab strip, but are not counted in the header total.
- (inferred) Antigravity agents are out of scope for v1: the interview fixed the tree on
  "Claude session", and they are not counted.
- (inferred) Clicking a pane or subagent node switches to its tab (pane focus inside
  tmux is not attempted).
- (inferred) "Live" means the panel reflects a state change on the same frame the
  sidebar card does — same source, same refresh.

## Research

### Status source (what the card reads)
- One agent per tab: `TabMeta { branch, agent: Option<Agent>, cwd, task, agent_pid }` (`crates/app/src/tabmeta/mod.rs:60`); `AgentState { Busy, Waiting, Idle, Unknown }` (`tabmeta/mod.rs:128`), `AgentKind { Claude, Antigravity, Codex, Gemini, Aider }` (`tabmeta/mod.rs:160`).
- Log classification: `state_from(last, role, mtime, now)` (`crates/app/src/tabmeta/activity.rs:66`) — `end_turn`/asks-user → `Waiting`; tool activity → `Busy`; otherwise a recency fallback (`BUSY_WINDOW` 10 s, `activity.rs:44`) → `Busy`/`Idle`. A finished Claude turn therefore reads **Waiting for input**, not Ready.
- Per-tab derivation `Deriver::tab` (`tabmeta/worker.rs:102`), `Deriver::activity` (`worker.rs:211`, Claude → `resolve_log` + `state_of_found`, `worker.rs:253`). Active tab every 500 ms tick, background tabs every 4th tick, staggered. Published via `TabMetaShared { generation, tabs }` (`tabmeta/mod.rs:196`); readers use `TabMetaHandle::sync` (`mod.rs:296`); `tabmeta::global()` (`mod.rs:429`).
- Screen override: `prompt::shows_blocking_prompt` (`crates/app/src/prompt.rs:78`) → `Session::blocking_dialog` (`session.rs:237`) → `SidebarRow.blocking_dialog`; joined by `effective_state` (`crates/app/src/sidebar_model.rs:194`) and `resolve_status(agent, blocking_dialog, running, last_exit) -> CardStatus` (`sidebar_model.rs:223`: Busy→Working, Idle→Ready, Waiting→WaitingForInput, Unknown→Hidden).
- The full per-tab join (status + line-1 title + cwd) is the private pure fn `row_meta(row, meta, home) -> RowMeta` (`sidebar_model.rs:67`), called from `SidebarMetaSource::fill` (`sidebar_model.rs:47`). Rows live in the thread-local `SidebarModel.rows` reachable via `TabModelHandle::shared()` (`crates/renderer/src/sidebar_row.rs:463,498`).
- `TabMeta`'s field set is locked by `tabmeta_still_has_exactly_its_five_published_fields` (`tabmeta/worker.rs:1122`); the resolved log path (`Found.log`) is not published.

### tmux
- No pane enumeration exists. `effective_root(memo, pid, tick)` (`crates/app/src/tabmeta/tmux.rs:220`) runs one `tmux list-clients -F '#{client_tty} #{pane_pid}'` per tick (memoised, `tmux.rs:154`) and `select_pane` (`tmux.rs:134`) maps the tab's tty to the attached client's **active** pane pid. The card therefore describes the active pane only; other panes are invisible today.

### Subagents
- Only the ai-monitor parses them, from the parent log: `ai_monitor::telemetry::Subagent { id, subagent_type, description, start_ms, end_ms, completed, agent_id, ended_ok, .. }` (`crates/ai-monitor/src/telemetry.rs:222`), opened by `Agent` tool_use, ended by a `<task-notification>` on `agent_id` (`telemetry.rs:1513`); flat list, capped at 32 (`TelemetrySnapshot.subagents`, `telemetry.rs:362`). Incremental API: `SessionTelemetry::apply(line)` (`telemetry.rs:591`), `backfill_telemetry(log) -> (SessionTelemetry, u64)` (`crates/ai-monitor/src/lib.rs:788`), `LogWatcher` (`lib.rs:829`).
- Subagent transcripts (`<encoded-cwd>/<session>/subagents/agent-<id>.jsonl`) are deliberately never watched (`specs/agent-elapsed-sidechain/spec.md:44,76`). Claude Code 2.1.28x writes a `tool_use` only once the tool returns (G8), so a foreground subagent is first seen already finished; a background launch returns at once, so it is seen running until its notification.

### Panel plumbing
- Overlay trait `crates/app/src/seam.rs:249` (`priority`, `visible`, `on_key(&KeyInput, &mut AppCtx)`, `on_wheel`, `on_press(point) -> Handled` with **no `AppCtx`** at `seam.rs:285`, `layer()`, `poll()`, `on_active_changed`). Registered in `feature_overlays` (`crates/app/src/lib.rs:160-190`); wake pattern = `Arc` closure sending `UserEvent::OverlayPending` (`lib.rs:168-173`). Registration counts asserted in `crates/app/tests/wiring.rs:48,66` (5 overlays, 2 handlers) plus priority/order checks (`wiring.rs:143-196`); `capture.rs:1592` parses `feature_overlays(` source.
- `AppRequest` (`seam.rs:137`) has `SetUserTitle`, `CloseTab`, `SetMonitorOpen`; no activate-tab variant. `apply_requests` (`terminal_app.rs:896`). Tab switching funnels through `do_tab_action(TabAction::Jump(i))` (`terminal_app.rs:1818`); `Tabs::jump(i)` (`tabs.rs:184`), `by_id` (`tabs.rs:87`).
- Clicks reach overlays only through `input::in_monitor_panel(x, viewport_w, right_inset)` (`input.rs:757`) → `overlays.press(p)` (`terminal_app.rs:2658-2669`); sidebar clicks go through `sidebar_press` → `press_route` (`input.rs:257`).
- Insets: `left_inset` (sidebar, `crates/renderer/src/lib.rs:685,631`), `right_inset` single-owner for the monitor (`lib.rs:693,732`, `monitor_band_px` `lib.rs:2047`), `grid_dims(.., top, left, right)` (`lib.rs:2001`). Tests `right_inset_reserves_cols_for_the_ai_monitor_panel` (`lib.rs:2189`).
- Monitor panel rendering pattern: `MonitorHandle` (`crates/renderer/src/monitor_panel.rs:56`, `press` at 203 hit-tests published bands), `MonitorPanel::record` two-stage cache (`monitor_panel.rs:636`), `monitor_layout::{layout, compose, TextRun, Primitives}` (`crates/renderer/src/monitor_layout.rs:94-482`), shapes via `SidebarShapePipeline` (`sidebar_shapes.rs:346`). `DrawLayer` z: glyph 0 < tab bar 10 < image 20 < overlay 100 (`renderer/src/lib.rs:176`).
- Pixel tests: `Renderer::capture` (`renderer/src/lib.rs:1187`), `renderer::testing::headless` (`testing.rs:145`), `crates/renderer/tests/golden_frame.rs` (sidebar cards at 657; no monitor-panel frame), `crates/app/tests/vertical_slice.rs`.
- Cmd+Shift chords taken: M (monitor), R (rename), G (capture), S (sidebar), C/V, T/W/[/] (shift-agnostic tab actions), A (in rename), =/+. Free: F, L, J, K, O, P, Y, U, I, B, D, H, X, Z. Overlays see keys before sidebar/font/tab chords (`terminal_app.rs:2272-2285`).
- Single window: one `App`, one `window` (`terminal_app.rs:161,203,409`).

## L2 Checkpoints (unattended)

Complexity: **Complex (4 signals)** — external resources (session `.jsonl` logs, `tmux`
processes); 3+ subsystems (tabmeta worker, ai-monitor telemetry, renderer panel, input
routing); async (worker thread + a viewer telemetry thread); multi-stage state (subagent
running → finished, panel open/closed, right-dock occupancy).
Brownfield weights: Core 30 · Scope 20 · Error 25 · Data 15 · Implementation 10.

| Dimension | Checkpoints | Resolved by |
|-----------|-------------|-------------|
| Core | tree shape; node states; header count; toggle; click target; subagent display; refresh cadence | request (D1–D3, D7, D8), assumed (D6, D11) |
| Scope | window/tab/pane scope; non-Claude agents; tabs without Claude; placement vs ai-monitor; multi-window; Antigravity | request + assumed (D4, D5, D9) |
| Error/Edge | tmux absent / not attached; unreadable or rotated log; many nodes (overflow); pane dialog unseen; telemetry cap; tab closed mid-watch; background subagent outlives turn | assumed (D10, D12–D15) |
| Data | per-pane data source; log path publication; subagent source; model + cache key | L1 + assumed (D9, D10, D11, D16) |
| Implementation | crate placement; overlay seam changes; tests (pixel + wiring + mutation) | L1 (D16, D17) |

Fenced checkpoints: none (no schema, migration, auth, payment or secret handling).
Composite after resolution: 1.00 (every checkpoint resolved; 13 of them `assumed`).
Inversion probe (answered from evidence): *every requirement met yet it fails* if the
panel's state for a split tab's **non-active** pane disagrees with what the user sees in
that pane — the card cannot see it either, so there is nothing to agree with; recorded as
D13 and a Known Gap rather than a promise. Implication probe: D4 (shared right dock)
also means toggling the flow panel closes an open ai-monitor and vice versa — recorded in D4.

## Decisions

### D1: Tree shape and node kinds
- **Status**: resolved (request)
- **Rationale**: `window → tab → pane (only when the tab's tmux window has ≥ 2 panes) →
  Claude session → subagent`, fixed in interview round 1–2. The window root is a single
  node (single-window app, `terminal_app.rs:161`). Rejected: grouping by project/cwd
  (the user's mental model is the tab strip), and showing tmux session/window levels
  (noise for the "how many / which are busy" question).

### D2: Session states are the card's states, computed by the card's own function
- **Status**: resolved (request) + assumed (mechanism)
- **Rationale**: Working / Waiting for input / Ready, and "must agree with the sidebar
  card". Agreement is guaranteed **by construction**: a tab's session node (or, in a
  split tab, its *active* pane's session node) takes its status from the same
  `row_meta(row, meta, home)` join the card uses (`sidebar_model.rs:67`, made `pub` and
  re-exported from the crate root per D17), fed by the same `TabMetaHandle` snapshot and the same
  `SidebarRow.blocking_dialog`. Rejected: re-deriving status in the viewer (two
  classifiers drift — the exact failure the user ruled out) and reading the renderer's
  private resolved `RowMeta` cache (not reachable from the app). Consequence carried
  forward: a finished Claude turn reads **Waiting for input** in the viewer exactly as on
  the card (`activity.rs:66`); the viewer does not "fix" that.

### D3: Header count format
- **Status**: resolved (request)
- **Rationale**: `{w} working · {q} waiting / {n}` where `n` counts every Claude session
  node (active-pane and non-active-pane sessions alike), `w` those Working and `q` those
  Waiting for input. Subagents are never counted (the user asked how many *sessions*).
  A session whose status resolves to `Hidden` (`AgentState::Unknown`) is still drawn as a
  session node, with no status dot and its label in the panel's muted text color, and
  counts in `n` only.
  The unit counted is one **Claude process** found under a tab or pane (one session node),
  not one log path: two panes whose resolution lands on the same `.jsonl` are two nodes and
  count 2 (see D18).
  Zero sessions renders `0 working · 0 waiting / 0`, not an empty header.

### D4: Placement — the flow panel shares the right dock with the ai-monitor
- **Status**: assumed
- **Rationale**: The right band (`right_inset`, `monitor_band_px`) gets one occupant at a
  time, `RightDock { None, Monitor, Flow }`, same band width as the monitor, so the grid
  reflows identically whichever occupies it. The width is the **dock's**, not the
  monitor's: `[ai_monitor].width_px` plus its runtime drag override size the band for
  either occupant, and dragging the band edge while Flow occupies it resizes that same
  shared width. Rejected: a fixed flow width (a second width config and a reflow on every
  swap).
  **Who owns the truth.** The app owns `RightDock`. Neither overlay flips its own
  visibility any more: the monitor's chord handler and the flow chord handler each queue a
  new `AppRequest::ToggleDock(Occupant)` (replacing the monitor's direct
  `SetMonitorOpen` from `on_key`); `apply_requests` computes the next state from the table
  below, sends a new app→overlay `OverlayCommand::DockChanged { dock, cwd, shell_pid }`
  (the active tab's `Tabs::active_cwd` / `active_shell_pid`, exactly what
  `restore_monitor_panel` hands `RestoreMonitor` today, because `on_command` gets no
  `AppCtx`), then reserves or frees the band through the existing
  `apply_monitor_open(dock != RightDock::None)` (`terminal_app.rs:1388`), whose renderer
  flag now means "right band reserved". `DockChanged` is **fanned out** to every overlay,
  like `Cancel` (`OverlayHost::command`'s `fan_out` match, `seam.rs:582`), because two
  overlays must both hear it. Each dock overlay sets `visible = (dock == its occupant)`
  there and nowhere else: the monitor runs its existing open path (`self.open(cwd,
  shell_pid)`) on becoming visible and its existing close path on becoming hidden, so a
  hidden monitor answers `visible() == false` and stops consuming Cmd+Opt+1..6 and wheel
  events. `RestoreMonitor` is retired: `DockChanged { dock: Monitor, .. }` carries the
  same cwd and pid and is the one open path.
  **Edge-triggered.** Because every overlay hears every `DockChanged`, each dock overlay
  runs its open path only on a hidden → visible change and its close path only on a
  visible → hidden change; a `DockChanged` that leaves its visibility as it was does
  nothing. The flow overlay's open path is `set_panes_wanted(true)` plus the telemetry
  spawn (D9, D10); both its tabmeta handle and its wake were handed to it at registration
  in `feature_overlays` (`FlowOverlay::new(wake, tabmeta, handle)`, T4), which is why
  `on_command`'s lack of an `AppCtx` does not matter.
  **Persistence.** Every applied `ToggleDock` calls `persist_monitor`, and
  `persist_monitor` (`terminal_app.rs:1503`) itself reads its open flag as
  `RightDock::persisted_open(self.dock)` (`dock == Monitor`) instead of
  `renderer.monitor_open()`, which is now true for Flow too — so every caller, the drag
  release in `finish_resize` included, persists the same intent. So Monitor → Flow → quit
  restarts with the dock empty, and the flow panel's state is never written.
  **Restore order** (`restore_monitor_panel`, `terminal_app.rs:1433`): set the app's dock
  to `RightDock::restored(cfg.ai_monitor.open)`; if that is `Monitor`, send
  `DockChanged { dock: Monitor, cwd, shell_pid }`, then `apply_monitor_open(true)` — the
  same command-then-band order the function uses today; no persist.
  Transition table (state × chord → next state):

  | state \ chord | Cmd+Shift+M | Cmd+Shift+F |
  |---|---|---|
  | None | Monitor | Flow |
  | Monitor | None | Flow |
  | Flow | Monitor | None |

  The monitor's persisted open flag restores to `Monitor` on start; the flow panel is
  never restored open (it is a glance surface, not a preference).
  **A width-forced close** (a window too narrow for the band, `sync_right_inset`'s R4.4
  squeeze) does not change the dock: like the monitor today, the occupant stays open in
  intent and reappears when the window widens, and the flow panel's pane work and
  telemetry thread keep running while squeezed. "Closed" in the Constraints means
  `dock != Flow`.
  **Rejected:**
  (a) a second right inset beside the monitor — two reserved bands starve the grid, and
  `Grid::resize` truncates rather than rewraps (G4), so opening both destroys wide rows
  twice; it also needs a second inset owner threaded through `sync_right_inset`/`grid_dims`;
  (b) **the band split vertically, tree above monitor** — the strongest alternative, since it
  needs no second inset and reads "alongside" literally. Rejected because the monitor's
  layout owns the band's full height: its item-ladder viewport, sticky slot and footer are
  laid out against the whole panel rect (`monitor_layout::layout(&model, panel, ..)`,
  `monitor_panel.rs:636`), and halving it halves the rows its six sections can show while
  the tree, which grows with every tab, competes for the same rows; a split would also
  need a second press router inside one band. It is the natural follow-up if the swap
  proves annoying, and is recorded in Known Gaps;
  (c) a floating overlay over the grid — covers terminal text, which the ai-monitor
  deliberately stopped doing (G4);
  (d) the left side next to the sidebar — the sidebar already owns `left_inset`, and the
  tree duplicates the card list there.
  The user said "alongside" the sidebar and ai-monitor, read here as *coexisting features*,
  not *simultaneously visible*; the swap is one chord, and clicking a node then
  Cmd+Shift+M shows that tab's telemetry.
  Clicks route for free: the right band is already the overlay press path
  (`input::in_monitor_panel`, `terminal_app.rs:2658`), and only the visible occupant
  answers `OverlayHost::press` (visible overlays only, `seam.rs:491`).

### D5: Scope — Claude sessions inside this terminal only
- **Status**: resolved (request) + assumed (details)
- **Rationale**: Only `AgentKind::Claude` agents found under this app's tabs/panes. Tabs
  whose agent is absent or another kind (Antigravity, Codex, …) are still listed as plain
  tab nodes with no session child, so the tree mirrors the tab strip, and contribute 0 to
  the count. Rejected: counting Antigravity (the tree was fixed on "Claude session"; its
  subagents come from a different telemetry path, `telemetry.rs:178`) and scanning
  `~/.claude/sessions` for machine-wide sessions (interview round 2 chose option a).

### D6: Toggle chord Cmd+Shift+F
- **Status**: assumed
- **Rationale**: "F" for flow; free per L1 (`capture.rs:530` letter census, no find
  feature exists). Matched case-folded like `is_toggle` (`overlay.rs:425`), and dispatched
  even while hidden, as the monitor's chord is (`OverlayHost::dispatch`, all overlays).
  Rejected: Cmd+Shift+L/J (no mnemonic), a menu item (no menu surface for overlays).

### D7: Click a node → switch to that node's tab
- **Status**: resolved (request) + assumed (pane/subagent)
- **Rationale**: Any tab, pane, session or subagent node activates its tab via a new
  `AppRequest::ActivateTab(TabId)`, applied by `apply_requests` through
  `do_tab_action(TabAction::Jump(i))` (`terminal_app.rs:1818`), so composition/rename/menu
  teardown happens exactly as for a sidebar click. The panel stays open. Pane nodes do
  **not** select the tmux pane (rejected: `tmux select-pane` mutates the user's tmux
  state from a viewer, and a viewer stays read-only towards its subjects). Clicking the
  window node or the header does nothing. Because `Overlay::on_press` has no `AppCtx`
  (`seam.rs:285`), its signature gains `ctx: &mut AppCtx`; the ai-monitor impl ignores it.
  Rejected: a pending-request side channel polled by the host (a second request path next
  to `AppCtx`).

### D8: Subagents — running listed, finished greyed behind `N finished`
- **Status**: resolved (request)
- **Rationale**: Under each Claude session: every running subagent as its own node (label
  = `subagent_type`, else `description`, else the literal string `agent`), then one `N finished` line when
  `N ≥ 1`. The line is collapsed by default; clicking it toggles the session's finished
  list open (greyed nodes) and closed — the click does not switch tabs. Fold state is
  keyed by the session node's identity `(TabId, pane_pid)` (`pane_pid` = `0` for an
  unsplit tab: no published `TabMeta` field carries the tab's root pid, and one session
  per unsplit tab makes `0` unique — as built, `flow.rs`), not by log path, so two nodes sharing a log
  fold independently (D18). Unlike the monitor's fold, whose state sits inside the
  renderer's `MonitorModel`, the fold **set** is owned by the app-side `FlowOverlay`
  (its key is an app `TabId`); the renderer's `FlowModel` carries only the result — an
  `expanded` flag per session row and the finished rows it implies. It still invalidates
  the layout stage, as a monitor fold does, because it changes which rows exist. An entry
  whose node disappears is dropped on the next rebuild. A subagent the log has not recorded yet is invisible; the parent
  simply shows its own status (round 3, option a). Rejected: a placeholder child (no
  evidence to place it from) and hiding finished subagents (round 5).

### D9: Panes — enumerated by the tabmeta worker, only while the panel is open
- **Status**: assumed
- **Rationale**: The worker already runs one memoised `list-clients` per tick
  (`tmux.rs:154`). It gains one memoised `tmux list-panes -a -F '#{session_name}:#{window_index}
  #{pane_active} #{pane_pid}'` per tick and extends the `list-clients` format with the
  client's `#{session_name}:#{window_index}`, so a tab's panes are the panes of the window
  its client shows. For each non-active pane it runs the same `agent_of_pid` →
  `resolve_log` → `state_of_found` chain as for the tab (`worker.rs:154,253`), on the
  tab's existing schedule. Published as `TabMeta.panes: Vec<PaneMeta { pane_pid, active,
  agent: Option<Agent>, log: Option<PathBuf> }>` — empty unless the window has ≥ 2 panes —
  and the tab-level resolved log as `TabMeta.log: Option<PathBuf>`; the field-lock test
  (`worker.rs:1122`) is updated to seven fields. Pane work runs **only while the flow
  panel is open**, toggled through the worker's command channel, so an app with the panel
  closed spends nothing new (G1). On the open command the worker runs one immediate pass
  over **every** tab (ignoring the background stagger) before resuming the schedule, so
  split tabs appear within one tick (≤ 500 ms) of opening; until that publish, split tabs
  render unsplit and the header's `n` may rise once.
  **Which pane is active** is decided by one source only: the pane whose `pane_pid` equals
  the `pane_pid` that `select_pane` returned from `list-clients` (the same pid the card's
  `TabMeta.agent` was derived from). `#{pane_active}` is not used for this choice, because
  the two process calls can disagree within a tick; a tick in which no listed pane matches
  publishes `panes` empty (the tab renders unsplit) rather than guessing. The active
  pane's `PaneMeta.agent` is filled with the same value as `TabMeta.agent`, but
  `build_flow` never reads it: the active pane has a session node iff `TabMeta.agent` is
  Claude, and its status comes from `row_meta` (D2). At most 8 panes per tab are derived (`MAX_PANES`).
  Rejected: a second, viewer-owned process scanner (two derivations of one pane would
  break D2 for the active pane), and per-tab `list-panes` calls (N processes per tick).

### D10: Subagent source — a viewer-owned telemetry thread over published logs
- **Status**: assumed
- **Rationale**: While the panel is open, one background thread holds, per distinct log path in `TabMeta.log` / `PaneMeta.log`, an
  `ai_monitor::SessionTelemetry` built by `backfill_telemetry(log)` and advanced by
  `LogWatcher`, and publishes `log → Vec<Subagent>` behind a generation counter; it wakes
  the loop with the overlay wake pattern (`lib.rs:168-173`). Logs no longer referenced are
  dropped on the next pass.
  **Lifecycle.** Spawned on open with a fresh `Arc<AtomicBool>` stop flag; on close the
  flag is set and the handle is dropped (detached) — the UI thread never joins, so closing
  during a large backfill cannot stall the event loop. The thread checks the flag between
  lines and between logs and exits on its own; a reopen spawns a new thread with a new flag
  while any old one winds down. The subagent **generation counter is an
  `Arc<AtomicU64>` owned by the overlay** and handed to each spawn, so it is monotonic
  across opens and D11's cache key can never match a model from a previous open.
  **Each spawn gets its own output slot** (`Arc<Mutex<HashMap<PathBuf, Vec<Subagent>>>>`),
  which the overlay replaces on reopen and reads only from the current spawn; the thread
  also re-checks its stop flag immediately before every publish and discards its work
  once it is set. A thread left over from a previous open can therefore neither write
  into the current slot nor bump the generation the current model is keyed on. Rejected: having the tabmeta worker parse subagents (it only
  tails the last lines for state; a full backfill per tab every tick is the wrong cost
  shape), and reading `subagents/agent-*.jsonl` transcripts (deliberately never watched,
  `specs/agent-elapsed-sidechain/spec.md:44`).

### D11: Refresh — rebuild the model only on change, same frame as the card
- **Status**: assumed
- **Rationale**: Each frame the panel compares `(tabmeta generation, rows_rev, subagent
  generation, fold state, scroll)` against its last key and rebuilds the `FlowModel` only
  when it changed; layout is cached on the model, compose on the scroll offset, mirroring
  `MonitorPanel::record`'s two stages (`monitor_panel.rs:636`). Because the session status
  comes from the same snapshot generation the card's `refresh_meta` pulls, a state change
  reaches both on the same frame. Rejected: a timer-driven refresh (second cadence, can
  lag the card) and an unconditional per-frame rebuild (reopens the G1 idle debt).

### D12: Overflow — rung-index scroll, no scissor
- **Status**: assumed
- **Rationale**: One row per node; when the tree is taller than the panel, the wheel
  scrolls by whole rows (the *item ladder* model, scroll offset outside the model,
  `domain.md`), clamped so the last row stays reachable. The header row is fixed above the
  list. Labels ellipsize to the band width. Rejected: pixel scrolling (needs a scissor
  rect the panel path does not have, C1).

### D13: Non-active pane sessions skip the dialog override
- **Status**: assumed
- **Rationale**: `blocking_dialog` is read off the tab's grid as one screen
  (`prompt.rs:78`); the card applies it to the tab, i.e. the active pane. The viewer
  applies it exactly there (D2) and gives non-active panes `resolve_status(pane.agent,
  false, false, None)` — log-only, exactly that expression. A pending permission dialog in
  a non-active pane is therefore invisible to the viewer. Rejected: per-pane screen scanning via `tmux capture-pane`
  (a process per pane per tick, and a second dialog matcher input).

### D14: Degradation without tmux or logs
- **Status**: assumed
- **Rationale**: No tmux server, a tab not attached to a client, or `list-panes` failing →
  `panes` empty, the tab renders as unsplit with the session from `TabMeta.agent`. A log
  path that is absent, unreadable or rotated (`/clear`) → that session shows its status with
  no subagent children until the next pass finds the new log; no error is drawn. A tab
  closed while its log is watched → its watcher is dropped on the next pass (D10).

### D15: Subagent cap is the telemetry's cap
- **Status**: assumed
- **Rationale**: `TelemetrySnapshot.subagents` retains at most 32 (`telemetry.rs:362`); the
  viewer shows what is retained and `N finished` counts retained finished records. Rejected:
  a second uncapped parser for an exact lifetime count (no requirement needs it).

### D16: Code placement
- **Status**: resolved (L1)
- **Rationale**: Renderer (window-agnostic): `crates/renderer/src/flow_layout.rs` (pure
  `FlowModel` → `Primitives`, `TextRun`/shape reuse from `monitor_layout`) and
  `crates/renderer/src/flow_panel.rs` (`FlowHandle` with `press` against published row
  bands, `FlowPanel: DrawLayer` at overlay z). App: `crates/app/src/flow.rs` (the
  `FlowOverlay` and the pure builder, whose one signature is
  `build_flow(rows: &[SidebarRow], metas: &HashMap<TabId, TabMeta>, home: Option<&Path>,
  subagents: &HashMap<PathBuf, Vec<Subagent>>, folds: &FoldSet) -> (FlowModel, FoldSet)`
  with `FoldSet = HashSet<(TabId, u32)>`; the returned `FoldSet` is the input with every
  entry whose session node no longer exists removed, and the overlay stores it as its new
  fold set (D8). The overlay fills `metas` from its own
  `TabMetaSnapshot` because the snapshot's fields are private) and
  `crates/app/src/flow_telemetry.rs` (the telemetry thread). `FlowModel` and its
  `header()` live in the renderer's `flow_layout.rs` — it is the renderer's input
  contract, like `SidebarRow`. The renderer's press answer is
  `FlowHandle::press(point) -> Option<FlowHit>` with `FlowHit::{Header, Node(u64),
  Fold(u64, u32)}` (raw tab id, pane pid); the overlay maps `Node(id)` to
  `AppRequest::ActivateTab(TabId(id))` and `Fold(id, pid)` to toggling `(TabId(id), pid)`
  in its own `FoldSet` — the toggle is app-side. tmux parsing additions in `tabmeta/tmux.rs`, pane derivation in
  `tabmeta/worker.rs`. Dependency direction holds: the renderer sees only plain data
  (`FlowModel` with `u64` tab ids, like `SidebarRow`), never app types.

### D17: Verification shape
- **Status**: resolved (L1 + CLAUDE.md)
- **Rationale**: Pure decisions (`build_flow`, count, fold, tmux parsers, dock transitions)
  get unit tests; the panel gets a golden-frame test through `Renderer::capture` asserting
  that header and node rows carry ink and that the three status colors reach the screen
  (CLAUDE.md: a render change is not covered until pixels are asserted); `wiring.rs`
  counts become 6 overlays; `cargo mutants -f` on each new file with 0 non-equivalent
  survivors.
  **Test-facing surface.** The app crate's modules are all `pub(crate)` and its public
  surface is the re-export list in `crates/app/src/lib.rs`, each line naming its consumer
  (`lib.rs:46-67`). The acceptance tests live in `crates/app/tests/tt*_*.rs` and
  `crates/renderer/tests/tt*_*.rs`, so every item a requirement's When names in the app
  crate is reachable from the crate root, re-exported by name there. The complete list:
  T1 — `parse_panes`, `PaneRow`, `parse_clients`, `ClientRow`, `select_pane`, `panes_of`,
  `MAX_PANES`, `pane_pass`, `PanePass`,
  `TabMeta`, `PaneMeta`, `Agent`, `AgentState`, `AgentKind`; T2 — `RightDock`, `Occupant`,
  `AppRequest`, `AppCtx`, `OverlayCommand`, `OverlayHost`, `Handled`, `KeyInput`,
  `AiMonitorOverlay`, `Tabs`, `tab_position`; T4 — `FlowOverlay`, `build_flow`, `FoldSet`, `row_meta`,
  `resolve_status`, `spawn_flow_telemetry` (`flow_telemetry::spawn` renamed on export),
  `FlowTelemetry`, `SubagentSlot`. Each
  item is made `pub` inside its `pub(crate)` module and re-exported. Types from other
  crates are named through those crates, which are ordinary dependencies of `app` and so
  visible to its tests: `renderer::sidebar_row::SidebarRow`, `renderer::sidebar_card::CardStatus`,
  `renderer::flow_layout::FlowModel`, `renderer::flow_panel::{FlowHandle, FlowHit,
  FlowView, scroll_offset}`, `ai_monitor::telemetry::Subagent`, `app::TabId`;
  no re-export is added for them. Every app re-export carries a `/// crates/app/tests/tt{n}_….rs` consumer line, in the
  task that creates the item. Renderer items live in new `pub mod flow_layout` /
  `pub mod flow_panel`; `FlowPanel` exposes `layout_count()` and `compose_count()` for R9.2–R9.3.
  Rejected: making modules `pub` (switches the dead-code lint off crate-wide, as `lib.rs:25-45` measured).

### D18: Remaining failure edges
- **Status**: assumed
- **Rationale**:
  - **Telemetry thread dies** (panic, or `backfill_telemetry` cannot open a log): the
    published map simply lacks that log; sessions still render with their status and no
    subagent children, and a `tracing::warn!` names the log once. The thread is not
    restarted until the panel is reopened. Rejected: drawing an error row (the panel is a
    glance surface; G7 shows a silent failure needs a log line, not UI).
  - **Worker command channel closed or the open command late**: `panes` stays empty and
    every tab renders unsplit — D14's degradation, not a new state.
  - **Two panes resolve to one log** (tier-3 "newest `.jsonl` in cwd", `activity.rs:224`):
    two session nodes (D3 counts processes), each showing that log's subagents. Rejected:
    deduplicating by log path (would hide a real process the user can see).
  - **Tab reorder/close between `rows_rev` and the tabmeta generation**: the tree's order
    and membership come from `SidebarModel.rows` alone; metadata is joined by `TabId`, so a
    row with no meta renders as a plain tab node and a meta with no row is ignored.

## Constraints
- `term-core` gains nothing; the renderer never imports app or winit types (dependency-direction).
- No GUI crates; the panel is drawn with the existing shape pipeline and glyph atlas (no-gui-crates).
- No new work while the panel is closed — `dock != Flow`, D4 (G1): pane enumeration is off, and the telemetry thread does no new work after close — it exits at its next line or log boundary and never publishes again.
- No per-frame model rebuild or reshape on an unchanged key (G1).
- A session node's status for a tab (unsplit) or its active pane (split) must come from `row_meta`, the card's join — never a second classifier.
- The viewer never mutates tmux or any session; it reads only.
- No `unwrap()` outside tests; errors via the crate's `thiserror` enum.
- macOS only, local zsh only; no new platform branches.

## Known Gaps
- D4 is the one assumption a human should confirm: the flow panel and ai-monitor **replace** each other in the right dock rather than both being visible.
- Non-active tmux pane sessions read `resolve_status(pane.agent, false, false, None)` (D13); a pending on-screen dialog in such a pane is invisible to the viewer.
- A foreground subagent is first visible only after it finishes (Claude Code 2.1.28x writes `tool_use` on return, G8); only background subagents are ever seen running.
- A squeezed (too-narrow window) flow panel keeps its pane work and telemetry thread running though nothing is drawn (D4); it costs only while the window is narrow.
- The stacked layout (tree above monitor in one band, D4 alternative b) is the follow-up if the dock swap proves annoying.
- `N finished` counts retained records; past 32 subagents per session older ones fall out (D15).
- The card's own model is one-session-per-tab; the viewer shows more sessions than the sidebar for split tabs, by design — the count is the viewer's, not the sidebar's.

## Requirements

### R0: A glance shows every Claude session in this terminal, by state, live — D1, D2, D3

#### R0.1: Working count changes live (manual)
- **Given**: the app runs with two tabs, each running `claude`, and the flow panel open (Cmd+Shift+F)
- **When**: the user sends a prompt in tab 1, waits for it to finish, then does the same in tab 2
- **Then**: the header's `working` number goes 0 → 1 → 0 → 1 → 0 in step with each tab's sidebar card turning Working and back, and each session node shows the same state as its card at every moment observed

### R1: The right dock has one occupant, switched by two chords — D4, D6

#### R1.1: Dock transition table
- **Given**: a `RightDock` in each of `None`, `Monitor`, `Flow`
- **When**: `RightDock::toggle(self, Occupant::Monitor)` and `RightDock::toggle(self, Occupant::Flow)` are called (on `app::RightDock`, re-exported per D17)
- **Then**: the results are exactly: None+M→Monitor, None+F→Flow, Monitor+M→None, Monitor+F→Flow, Flow+M→Monitor, Flow+F→None

#### R1.2: Cmd+Shift+F queues a dock toggle
- **Given**: a hidden `FlowOverlay::new(wake, None, FlowHandle::default())` (D16's constructor; no tabmeta handle)
- **When**: `Overlay::on_key` receives Cmd+Shift+F (and, separately, Cmd+Shift+f with caps differences)
- **Then**: it returns `Handled::Consumed` and the `AppCtx` holds exactly one `AppRequest::ToggleDock(Occupant::Flow)`; any other chord returns `Passthrough` and queues nothing

#### R1.3: The monitor's chord goes through the dock
- **Given**: an `AiMonitorOverlay`
- **When**: `Overlay::on_key` receives Cmd+Shift+M
- **Then**: the `AppCtx` holds `AppRequest::ToggleDock(Occupant::Monitor)` and no `SetMonitorOpen` request, and the overlay's `visible()` is unchanged until a `DockChanged` arrives

#### R1.4: A hidden monitor stops consuming its keys
- **Given**: an `AiMonitorOverlay` made visible by `on_command(&OverlayCommand::DockChanged { dock: RightDock::Monitor, cwd: None, shell_pid: None })`
- **When**: it receives `on_command(&OverlayCommand::DockChanged { dock: RightDock::Flow, cwd: None, shell_pid: None })` and then Cmd+Opt+1 through `on_key`
- **Then**: `visible()` is false and Cmd+Opt+1 returns `Passthrough`

#### R1.5: The flow overlay follows the dock
- **Given**: a `FlowOverlay::new(wake, None, FlowHandle::default())`
- **When**: it receives `DockChanged` with `dock` = `Flow`, then `Monitor`, then `None`
- **Then**: `visible()` reads true, false, false

#### R1.8: Both dock overlays hear one DockChanged
- **Given**: an `OverlayHost` holding an `AiMonitorOverlay` (priority 100) and a `FlowOverlay`
- **When**: `OverlayHost::command(&OverlayCommand::DockChanged { dock: RightDock::Flow, cwd: None, shell_pid: None })` is called
- **Then**: the flow overlay's `visible()` is true and the monitor's is false — the command reached both, whatever either returned

#### R1.9: Only the monitor's state is persisted
- **Given**: each `RightDock` value
- **When**: `RightDock::persisted_open(self)` is called
- **Then**: it is true only for `Monitor`, so the sequence Monitor → Flow persists `open = false` and `RightDock::restored(false)` restarts as `None`

#### R1.6: Applying a toggle reserves the band and fans out (manual)
- **Given**: the running app with the ai-monitor open
- **When**: the user presses Cmd+Shift+F
- **Then**: the ai-monitor panel disappears, the flow panel draws in the same right band at the same width, and the terminal grid's column count is unchanged; Cmd+Shift+F again leaves the band empty and the grid widens

#### R1.7: Restore opens the monitor, never the flow panel
- **Given**: `[ai_monitor].open` read from config at start
- **When**: `RightDock::restored(monitor_open: bool)` (on `app::RightDock`, which the startup restore uses) is called with `true` and with `false`
- **Then**: it returns `Monitor` and `None` respectively; it has no input that yields `Flow`

### R2: The tree mirrors the tab strip — D1, D5, D18

#### R2.1: Unsplit tab with Claude
- **Given**: one `SidebarRow` for tab A and a `TabMeta` whose `agent` is `Claude` and `panes` is empty
- **When**: `app::build_flow(rows, metas, home, subagents, folds)` is called (signature fixed in D16)
- **Then**: the model is window root → tab A → one session node, with no pane node

#### R2.2: Split tab shows one pane node per pane
- **Given**: tab B whose `TabMeta.agent` is `Claude/Busy` and whose `TabMeta.panes` holds `{4100, active: true, Claude/Busy}`, `{4200, active: false, Claude/Idle}` and `{4300, active: false, agent: None}`
- **When**: `build_flow` is called
- **Then**: tab B has three pane nodes in the published order; panes 4100 and 4200 each have one session child and pane 4300 has none

#### R2.3: Tabs without a Claude agent are plain nodes
- **Given**: tab C with `agent: None` and tab D with an `Antigravity` agent
- **When**: `build_flow` is called
- **Then**: both appear as tab nodes with no children

#### R2.4: Rows own order and membership
- **Given**: rows `[A, B]`, metas for `B` and a closed tab `Z` but not `A`
- **When**: `build_flow` is called
- **Then**: the tree lists A then B; A is a plain tab node; Z does not appear

#### R2.5: Tab labels are the card's line 1
- **Given**: a row with a user label `"api"` and a meta with a task label
- **When**: `build_flow` is called
- **Then**: the tab node's label equals `row_meta(row, meta, home).line1` (`"api"`)

### R3: Session status equals the card's — D2, D13

#### R3.1: Unsplit session takes the card's status
- **Given**: for each `AgentState` value and both `blocking_dialog` values, a row and a Claude meta
- **When**: `build_flow` is called
- **Then**: the session node's status equals `row_meta(row, meta, home).status` in every combination (Busy+dialog → WaitingForInput included)

#### R3.2: Active pane takes the card's status, chosen by pid
- **Given**: a split tab whose row has `blocking_dialog: true`, whose `TabMeta.agent` is `Claude/Busy`, and whose panes are `{pane_pid 10, active: true}` and `{pane_pid 11, active: false, Claude/Busy}`
- **When**: `build_flow` is called
- **Then**: pane 10's session reads `WaitingForInput` (the card's status) and pane 11's session reads `Working`

#### R3.3: Non-active pane is log-only
- **Given**: a non-active `PaneMeta` with `Claude/Waiting` and a row with `blocking_dialog: false`
- **When**: `build_flow` is called
- **Then**: its session status equals `resolve_status(Some(&agent), false, false, None)` (`WaitingForInput`)

#### R3.4: Unknown state renders as an undotted session
- **Given**: a Claude meta with `AgentState::Unknown`
- **When**: `build_flow` is called
- **Then**: a session node exists with status `Hidden`

### R4: Header counts sessions by state — D3, D18

#### R4.1: Count format
- **Given**: five session nodes: two Working, one WaitingForInput, one Ready, one Hidden
- **When**: `renderer::flow_layout::FlowModel::header()` is read on the model `build_flow` returned
- **Then**: it is exactly `2 working · 1 waiting / 5`

#### R4.2: Zero sessions
- **Given**: rows with no Claude agents
- **When**: `build_flow`, then `FlowModel::header()` on its model
- **Then**: `0 working · 0 waiting / 0`

#### R4.3: Processes, not logs, are counted
- **Given**: two non-active panes whose `PaneMeta.log` is the same path, both Claude/Busy
- **When**: `build_flow`, then `FlowModel::header()` on its model
- **Then**: `n` includes both (two session nodes, `2 working`)

#### R4.4: Subagents and other agents are not counted
- **Given**: one Claude session with three running subagents, and an Antigravity tab
- **When**: `build_flow`, then `FlowModel::header()` on its model
- **Then**: `n` is 1

### R5: Subagents under their session, finished ones folded — D8, D15

#### R5.1: Running subagents are listed with a label
- **Given**: a session whose log maps to subagents `{type: "Explore", completed: false}`, `{type: None, description: "fix", completed: false}`, `{type: None, description: None, completed: false}`
- **When**: `build_flow` is called with that `subagents` map
- **Then**: three running subagent nodes labelled `Explore`, `fix`, `agent`, in log order

#### R5.2: Finished subagents collapse behind one line
- **Given**: the same session plus four `completed: true` subagents and an empty fold set
- **When**: `build_flow` is called
- **Then**: after the running nodes there is exactly one `4 finished` node and no finished subagent nodes; with zero finished there is no such line

#### R5.3: The fold expands to greyed nodes
- **Given**: the fold set contains that session's `(TabId, pane_pid)`
- **When**: `build_flow` is called
- **Then**: the `4 finished` node is followed by four subagent nodes marked finished (rendered muted)

#### R5.4: Folds are per node and pruned
- **Given**: two session nodes sharing one log, with only the first's key in the fold set
- **When**: `build_flow` is called, then called again after the first node's pane disappears
- **Then**: only the first is expanded; after the second call the returned fold set no longer holds the vanished key

#### R5.5: Clicking `N finished` toggles the fold, not the tab
- **Given**: a visible `FlowOverlay` built on a `FlowHandle` on which the test called `publish_view(FlowView { rows: vec![(FlowHit::Fold(7, 10), [x0, y0, x1, y1])], visible_rows: 20 })`
- **When**: `Overlay::on_press(point_inside_that_rect, ctx)` is called on it, twice
- **Then**: each call returns `Consumed` and queues no `ActivateTab`; `FlowOverlay::folds()` contains `(TabId(7), 10)` after the first call and not after the second

### R6: The worker publishes panes while the panel is open — D9, D14

#### R6.1: Parse `list-panes`
- **Given**: `list-panes -a` output lines `work:1 1 4100`, `work:1 0 4200`, `misc:0 1 5000` and a malformed line
- **When**: `app::parse_panes(&str)` is called
- **Then**: it returns three `(window "work:1"/"misc:0", active, pid)` rows and skips the malformed line

#### R6.2: Parse the extended `list-clients`
- **Given**: output `/dev/ttys004 4100 work:1` and a line missing its window column
- **When**: `app::parse_clients(&str)` is called
- **Then**: it yields exactly one `ClientRow { tty: "/dev/ttys004", pane_pid: 4100, window: "work:1" }`, and `select_pane(&[(4, 4100)], 4)` — the device given explicitly, as `resolve_rdevs` produces it — returns `Some(4100)`

#### R6.3: A tab's panes are its client's window, active by pid
- **Given**: the `ClientRow` above and `parse_panes` rows (`PaneRow { window, active, pid }`) for `work:1` with pids 4100 and 4200
- **When**: `app::panes_of(Some(&client), &panes)` is called
- **Then**: it returns pids `[4100, 4200]` with `active` true only for 4100 (the client pid), whatever `#{pane_active}` said; if no pane pid equals the client pid it returns empty

#### R6.4: Single-pane and oversized windows
- **Given**: a window with one pane, and another with ten
- **When**: `panes_of` is called on each
- **Then**: the first yields empty; the second yields exactly `MAX_PANES` (8), the active one always included

#### R6.5: `TabMeta` publishes panes and log
- **Given**: the `TabMeta` struct
- **When**: the field-lock test enumerates its fields
- **Then**: it has exactly seven: `branch, agent, cwd, task, agent_pid, panes, log`

#### R6.6: Panes only while wanted
- **Given**: the worker's pure pane-gating decision `app::pane_pass(wanted: bool, just_enabled: bool, due: bool) -> PanePass` (`PanePass { Skip, Derive }`), where `due` is the tab's ordinary schedule slot and `just_enabled` is true on the first tick after `TabMetaHandle::set_panes_wanted(true)`
- **When**: it is called for all eight input combinations
- **Then**: `wanted == false` → `Skip` always (no `list-panes` is run for that tick); `wanted && just_enabled` → `Derive` regardless of `due` (the immediate full pass); `wanted && !just_enabled` → `Derive` iff `due`

#### R6.7: No tmux degrades to unsplit
- **Given**: the worker maps a failed `list-panes` (non-zero exit, no server) to an empty pane list and a tab with no matching tmux client to `None`
- **When**: `app::panes_of(None, &rows)` and `app::panes_of(Some(&client), &[])` are called
- **Then**: both return empty, so that tab's `TabMeta.panes` is published empty and it renders unsplit

### R7: A viewer telemetry thread maps logs to subagents — D10, D18

#### R7.1: Referenced logs are backfilled and tailed
- **Given**: two `.jsonl` fixtures with `Agent` tool_use lines, handed to `app::spawn_flow_telemetry(logs: Vec<PathBuf>, slot: SubagentSlot, generation: Arc<AtomicU64>, wake: Arc<dyn Fn() + Send + Sync>) -> FlowTelemetry`
- **When**: the thread runs and one fixture is appended a `<task-notification>` for its agent
- **Then**: the slot maps each path to its `Vec<Subagent>`, the appended agent becomes `completed`, the generation increased, and `wake` was called

#### R7.2: Unreferenced logs are dropped
- **Given**: a running spawn watching logs `{a, b}`
- **When**: `FlowTelemetry::update_logs(&self, vec![a])` is called
- **Then**: the next publish has no entry for `b`

#### R7.3: Stop never blocks and never publishes late
- **Given**: a spawn backfilling a large fixture
- **When**: `FlowTelemetry::stop(&self)` is called on it and, immediately, a new spawn with a new slot and the same generation `Arc` starts
- **Then**: `stop` returns without joining; the old handle's `is_finished()` becomes true within 2 s; the new slot never contains entries written by the old thread; and after `stop` returned the old thread never publishes into its own slot again

#### R7.4: Unreadable log is absent, not fatal
- **Given**: a log path that does not exist
- **When**: the thread processes it
- **Then**: the slot has no entry for it, other logs still publish, and the thread keeps running (a later-created file at that path is picked up on a following pass)

### R8: Clicking a node switches to its tab — D7

#### R8.1: Panel hit-test
- **Given**: a `renderer::flow_panel::FlowHandle` (created by T3) with published row bands for a header, a tab row, a session row and an `N finished` row
- **When**: `FlowHandle::press([x, y])` is called inside each, and outside all
- **Then**: it returns `Header`, `Node(tab_id)`, `Node(tab_id)`, `Fold(key)` and `None` respectively

#### R8.2: Overlay press queues ActivateTab
- **Given**: a visible `FlowOverlay` whose `FlowHandle` was given `publish_view(FlowView { rows: vec![(FlowHit::Header, header_rect), (FlowHit::Node(7), row_rect)], visible_rows: 20 })`
- **When**: `Overlay::on_press(point, ctx)` is called
- **Then**: it returns `Consumed` and `ctx` holds exactly `AppRequest::ActivateTab(TabId(7))`; a `Header` hit returns `Consumed` with no request

#### R8.3: ActivateTab resolves a tab id to its strip index
- **Given**: the tab ids `[TabId(1), TabId(7), TabId(9)]` in strip order
- **When**: `app::tab_position(ids: &[TabId], id: TabId) -> Option<usize>` (the pure lookup `Tabs::position` delegates to, and that `apply_requests` uses before `do_tab_action(TabAction::Jump(i))`) is called with `TabId(7)` and with `TabId(4)`
- **Then**: it returns `Some(1)` and `None`; on `None` `apply_requests` does nothing

#### R8.4: `on_press` carries the context
- **Given**: an `OverlayHost` with one visible test overlay whose `on_press` queues `ActivateTab(TabId(3))`, and one hidden overlay
- **When**: `OverlayHost::press(point, &mut ctx)` is called
- **Then**: `ctx.take_requests()` returns exactly `[ActivateTab(TabId(3))]` and the hidden overlay's `on_press` was not called; the ai-monitor's fold click still returns `Consumed` and queues nothing

### R9: The panel draws the tree and redraws only on change — D11, D12, D16, D17

#### R9.1: Pixels reach the screen
- **Given**: a headless renderer (`renderer::testing::headless`) with a `FlowPanel` layer holding a model with a header, one Working, one WaitingForInput and one Ready session
- **When**: `Renderer::capture` is called
- **Then**: the header row and every node row carry glyph ink in the panel band, and each session row's status dot area contains its status color (the same colors the sidebar card uses for those states)

#### R9.2: Unchanged key, no rebuild
- **Given**: a `FlowPanel` that has recorded one frame
- **When**: ten more frames record with the same model, fold set and scroll
- **Then**: its layout counter stays at 1

#### R9.3: A change rebuilds on the next frame
- **Given**: the same panel
- **When**: the model's one session changes status, then the scroll offset changes
- **Then**: the layout counter becomes 2 on the first change; the scroll change re-composes without a layout (layout counter stays 2, compose counter increases)

#### R9.4: Overflow scrolls by rows and clamps
- **Given**: 60 list rows and 20 visible rows
- **When**: `renderer::flow_panel::scroll_offset(current: usize, notches: i32, total: usize, visible: usize) -> usize` (the one function `FlowOverlay::on_wheel` uses) is called from 0 with +100, then from that result with -100, and with `total < visible`
- **Then**: it returns 40, then 0, and 0 when everything fits; and a `Renderer::capture` with `FlowHandle::set_scroll(40)` still shows ink in the header row

#### R9.5: Long labels ellipsize
- **Given**: a tab label of 200 characters
- **When**: `flow_layout::layout` lays it out in a band 30 cells wide
- **Then**: the run ends with `…` and no glyph of it falls outside the band

## Tasks

### T1: tmux pane enumeration and per-pane derivation in the tabmeta worker [infra]
- **Fulfills**: R6
- **Depends on**: (none) ← parallel with T3
- **Files**: `crates/app/src/tabmeta/tmux.rs` (`parse_panes -> Vec<PaneRow>`,
  `parse_clients -> Vec<ClientRow>` with the `#{session_name}:#{window_index}` column and
  `resolve_rdevs` adapted to it, `panes_of`, `MAX_PANES`, one memoised
  `list-panes -a` per tick), `crates/app/src/tabmeta/worker.rs` (pane derivation through
  the existing `agent_of_pid` → `resolve_log` → `state_of_found` chain, `pane_pass`,
  publishing `TabMeta.log` from the resolved `Found.log`, the field-lock test moved to
  seven fields), `crates/app/src/tabmeta/mod.rs` (`PaneMeta`, `PanePass`,
  `TabMeta.{panes, log}`, `TabMetaHandle::set_panes_wanted(bool)` over the existing
  command channel), `crates/app/src/lib.rs` (re-exports per D17).
- **Notes**: D9's active-pane rule (pid match, never `#{pane_active}`) and D14's
  degradation. The card's `agent` derivation must be byte-for-byte unchanged — existing
  tabmeta tests stay green untouched.

### T2: Right-dock ownership, `ActivateTab`, and `on_press` context [infra]
- **Fulfills**: R1, R8
- **Depends on**: T1 (both edit `crates/app/src/lib.rs`)
- **Files**: `crates/app/src/seam.rs` (`RightDock`, `Occupant`, `RightDock::toggle`,
  `RightDock::restored`, `AppRequest::{ToggleDock, ActivateTab}`,
  `RightDock::persisted_open`, `OverlayCommand::DockChanged { dock, cwd, shell_pid }` added
  to `OverlayHost::command`'s fan-out match beside `Cancel`, `RestoreMonitor` removed,
  `Overlay::on_press(&mut self, point, ctx: &mut AppCtx)`, `OverlayHost::press(point, ctx)`),
  `crates/app/src/overlay.rs` (the monitor's chord queues `ToggleDock(Monitor)` instead of
  `SetMonitorOpen`; `visible` follows `DockChanged` only, running its existing open path
  with the command's cwd/pid only on a hidden → visible change and its close path only on
  a visible → hidden change),
  `crates/app/src/terminal_app.rs` (the app owns a `dock: RightDock` field;
  `apply_requests`: `ToggleDock` → `toggle` → `DockChanged` → `apply_monitor_open(dock !=
  None)` → `persist_monitor`, which now reads `persisted_open(self.dock)` itself for every
  caller; `ActivateTab` →
  `Tabs::position` → `do_tab_action(TabAction::Jump(i))`; `restore_monitor_panel` in D4's
  restore order; the mouse press site builds an `AppCtx` the way the key path does
  (`terminal_app.rs:2203`), calls `overlays.press(p, &mut ctx)`, then `take_requests()`
  and hands them to `apply_requests`, so an `ActivateTab` is never dropped),
  `crates/app/src/tabs.rs` (pure `tab_position(&[TabId], TabId)` and `Tabs::position` over it), `crates/app/src/lib.rs` (re-exports).
- **Notes**: no renderer change — the band width and inset are the existing monitor
  band (D4). Any `SetMonitorOpen` producers left after the migration are removed, not
  kept alongside. Existing ai-monitor tests that assert `SetMonitorOpen` from `on_key`
  are updated to the dock request in this task (that is a behaviour change D4 makes,
  not a new test case).

### T3: Renderer flow panel — model, layout, press, cached record [infra]
- **Fulfills**: R4, R8, R9
- **Depends on**: (none) ← parallel with T1, T2
- **Files**: `crates/renderer/src/flow_layout.rs` (new: plain-data `FlowModel`,
  `FlowRow { depth, kind: Header|Window|Tab|Pane|Session|Subagent|Fold, label, status:
  Option<CardStatus>, muted, target: Node(u64)|Fold(u64, u32)|None }`, `layout(model,
  panel, scale, cell) -> PanelDraw` reusing `monitor_layout::{TextRun, Primitives}`,
  ellipsis), `crates/renderer/src/flow_panel.rs` (new: `FlowHandle` — `Clone + Default`, shared
  like `MonitorHandle` — with `set(FlowModel)`, `set_scroll(usize)`,
  `publish_view(FlowView { rows: Vec<(FlowHit, [f32; 4])>, visible_rows: usize })` (window-
  absolute rects, written by the panel's record and callable by tests), `view() ->
  FlowView` and `press([f32; 2]) -> Option<FlowHit>` with `FlowHit::{Header, Node(u64),
  Fold(u64, u32)}`; the pure `scroll_offset(current, notches, total, visible)`; `FlowPanel: DrawLayer` at `MONITOR_PANEL_Z`, two-stage cache keyed on
  model/rect/metrics/atlas then scroll, `layout_count()`/`compose_count()`),
  `FlowModel::header() -> String` (D3's format, counted from the session rows' statuses),
  `FlowPanel::new(handle: FlowHandle)` — the layer reads its model and scroll from the
  handle and draws in the right band `FrameCtx` gives it (`[viewport_w - right_inset,
  viewport_w]`, below `top_inset`), so a headless test reserves the band with
  `Renderer::set_monitor_open(true)` and adds the layer with `Renderer::add_layer`,
  `crates/renderer/src/lib.rs` (`pub mod` lines only).
- **Notes**: status colors come from the sidebar card's palette for the same
  `CardStatus` (`sidebar_card.rs`, `sidebar_theme.rs`), so a Working dot is the card's
  Working color; `Hidden` draws no dot. Rung-index scroll as in `monitor_scroll`
  (D12). Renderer stays window-agnostic and app-type-free (tab ids are `u64`).

### T4: Flow overlay — tree builder, subagent thread, registration [vertical]
- **Fulfills**: R0, R1, R2, R3, R4, R5, R7, R8, R9
- **Depends on**: T1, T2, T3, TT1, TT2, TT3, TT4, TT5, TT7, TT8, TT9
- **Files**: `crates/app/src/flow.rs` (new: `FlowOverlay::new(wake: Arc<dyn Fn() + Send +
  Sync>, tabmeta: Option<TabMetaHandle>, handle: FlowHandle)` — `feature_overlays` passes
  the capture-style wake, `tabmeta::global()` and a fresh handle; with `tabmeta: None` it
  builds from rows only and never sends `set_panes_wanted`; `folds() -> &FoldSet`;
  Cmd+Shift+F → `ToggleDock(Flow)`,
  `visible` from `DockChanged`, on open `set_panes_wanted(true)` + telemetry spawn, on close
  the reverse; per-frame key check and `FlowHandle::set` only on change (D11); `on_press`
  → `ActivateTab` or fold flip; `on_wheel` → scroll; pure `build_flow` with D16's
  signature),
  `crates/app/src/flow_telemetry.rs` (new: `SubagentSlot = Arc<Mutex<HashMap<PathBuf,
  Vec<Subagent>>>>`; `spawn(logs, slot, generation, wake) -> FlowTelemetry` per D10's
  lifecycle; `FlowTelemetry::{update_logs(&self, Vec<PathBuf>), stop(&self),
  is_finished(&self) -> bool}`, `Drop` calls `stop`),
  `crates/app/src/sidebar_model.rs` (`row_meta` and `resolve_status` made `pub` for the
  crate-root re-export, no behaviour change),
  `crates/app/src/lib.rs` (`feature_overlays` gains `FlowOverlay` with the capture-style
  wake; re-exports), `crates/app/tests/wiring.rs` (6 overlays, the flow overlay's priority
  assertion; `capture.rs`'s source parse of `feature_overlays(` kept passing),
  `docs/architecture.md` (a G-entry: the dock is shared, non-active panes are log-only,
  foreground subagents appear on return) and `docs/domain.md` (vocabulary: *flow panel*,
  *right dock*).
- **Notes**: the session status must be read through `row_meta` (Constraint); no
  second classifier. The flow overlay's priority sits next to the monitor's (100) and
  its chord is matched even while hidden, like the monitor's.

### TT1: acceptance tests for R1's GWTs [test]
- **Fulfills**: R1 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/app/tests/tt1_dock.rs` (R1.1–R1.5, R1.7–R1.9; R1.6 is manual)

### TT2: acceptance tests for R2's GWTs [test]
- **Fulfills**: R2 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/app/tests/tt2_flow_tree.rs`

### TT3: acceptance tests for R3's GWTs [test]
- **Fulfills**: R3 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/app/tests/tt3_flow_status.rs`

### TT4: acceptance tests for R4's GWTs [test]
- **Fulfills**: R4 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/app/tests/tt4_flow_header.rs`

### TT5: acceptance tests for R5's GWTs [test]
- **Fulfills**: R5 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/app/tests/tt5_flow_subagents.rs`

### TT6: acceptance tests for R6's GWTs [test]
- **Fulfills**: R6 (tests)
- **Depends on**: T1
- **Files**: `crates/app/tests/tt6_tmux_panes.rs`

### TT7: acceptance tests for R7's GWTs [test]
- **Fulfills**: R7 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/app/tests/tt7_flow_telemetry.rs` (fixtures written to a temp dir by the test)

### TT8: acceptance tests for R8's GWTs [test]
- **Fulfills**: R8 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/renderer/tests/tt8_flow_press.rs` (R8.1), `crates/app/tests/tt8_flow_click.rs` (R8.2–R8.4)

### TT9: acceptance tests for R9's GWTs [test]
- **Fulfills**: R9 (tests)
- **Depends on**: T1, T2, T3
- **Files**: `crates/renderer/tests/tt9_flow_panel.rs` (all of R9, R9.4's `scroll_offset` included, since it lives in the renderer; GPU-backed, skips when no adapter per the project's skip-vs-fail rule)

## External Dependencies

### Pre-work
- (none)

### Post-work
- Manual checks R0.1 and R1.6 in the running app (`/run-term`), with two tabs running `claude` and one tab split in tmux.
- `cargo mutants -f` on `crates/app/src/flow.rs`, `crates/app/src/flow_telemetry.rs`, `crates/renderer/src/flow_layout.rs`, `crates/renderer/src/flow_panel.rs` and the changed `crates/app/src/tabmeta/tmux.rs`; record survivor counts (CLAUDE.md).

## Spec Review
- **Verdict**: NEEDS_FIX
- **Reviews**: 3 (the maximum; the unattended run stops here and does not hand off)
- **Fixed** (between reviews, then re-reviewed):
  - [Grounding] R4.x/T3/T4 — `FlowModel::header()` moved into the renderer's `flow_layout.rs` (T3 now fulfills R4).
  - [Readiness] D17 — complete crate-root re-export list for every item a test names; cross-crate types named through their own crates.
  - [Grounding] R2.1, R6.x, R7.1 — paths rewritten as crate-root re-exports (`app::build_flow`, `app::parse_panes`, …).
  - [Readiness] D16/R2.1/T4 — one `build_flow` signature, with `home` and a concrete `metas` type; returned `FoldSet` defined.
  - [Task order] TT6 — now depends on T1.
  - [Testability] R6.6/R6.7 — pane gating and no-tmux degradation moved to pure functions (`pane_pass`, `panes_of`).
  - [Testability] R7.4 — dropped the unobservable "warning logged" clause.
  - [Readiness] D4/T2 — `DockChanged { dock, cwd, shell_pid }` fanned out like `Cancel`; `RestoreMonitor` retired; persistence via `RightDock::persisted_open` read inside `persist_monitor`; restore order spelled out; press site builds an `AppCtx` and applies its requests; edge-triggered open/close; width-forced squeeze keeps the dock.
  - [Readiness] T4/R1.x/R5.5/R8.2/R9.4 — `FlowOverlay::new(wake, tabmeta, handle)`, `FlowHandle::publish_view(FlowView)`, `folds()`, pure `scroll_offset`.
  - [Readiness] R7.x — `FlowTelemetry::{update_logs, stop, is_finished}` and `SubagentSlot` named.
  - [Testability] R6.2 — `ClientRow` and an explicit device for `select_pane`.
- **Open** (raised by the 3rd review; fixes were applied to this file afterwards but **no review has checked them**):
  - [Coverage] TT1 — scope now lists R1.8–R1.9.
  - [Testability] R8.3 — restated on a pure `tab_position(&[TabId], TabId)`.
  - [Testability] R2.2 — Given now names the active pane and `TabMeta.agent`; D9 states `build_flow` never reads the active pane's `PaneMeta.agent`.
  - [Readiness] D8/R5.1 — fallback label is the literal string `agent`.
  - [Readiness] T3/R9.x — `FlowPanel::new(handle)` and how a headless test reserves the band.
  - [Consistency] D2 — `row_meta` is `pub` and re-exported, matching D17.

## Implementation Notes (as built)

### T3 — renderer (`crates/renderer/src/flow_layout.rs`, `flow_panel.rs`)
- Types: `FlowKind { Header, Window, Tab, Pane, Session, Subagent, Fold }`, `FlowTarget { Node(u64), Fold(u64, u32), None }`, `FlowRow { depth, kind, label, status: Option<CardStatus>, muted, target }`, `FlowModel { rows }` with `header()` and `list_len()`; `status_color(CardStatus) -> Option<[f32; 4]>` (card palette, `None` for `Hidden`); `layout(model, panel, scale, cell) -> PanelDraw`; `compose(&PanelDraw, top) -> FlowFrame`.
- `FlowHit { Header, Node(u64), Fold(u64, u32) }`, `FlowView { rows: Vec<(FlowHit, [f32; 4])>, visible_rows }`; `FlowHandle` (`Clone + Default`): `set`, `model`, `set_scroll`, `scroll`, `set_visible(bool)`, `visible`, `publish_view`, `view`, `press`; `scroll_offset(current, notches, total, visible)`; `FlowPanel::new(FlowHandle)` with `layout_count()` / `compose_count()`.
- **Visibility is a handle flag** (`set_visible`), default **shown**; T4 must call `set_visible(dock == Flow)` or both panels paint the shared band. A hidden panel does no work and clears its press bands.
- Rects are corners `[x0, y0, x1, y1]`, window-absolute. Positive `notches` = down the list (later rows).
- A `Header` row inside `rows` is skipped; header text always comes from `header()`. `list_len()` is the `total` for `scroll_offset`.
- The view is republished on every compose (scroll moves the press bands).

### T1 — tabmeta panes (`crates/app/src/tabmeta/{tmux,worker,mod}.rs`)
- Crate-root exports: `MAX_PANES` (8), `ClientRow { tty, pane_pid, window }`, `PaneRow { window, active, pid }`, `parse_clients(&str) -> Vec<ClientRow>`, `parse_panes(&str) -> Vec<PaneRow>`, `select_pane(&[(u32, u32)], u32) -> Option<u32>`, `panes_of(Option<&ClientRow>, &[PaneRow]) -> Vec<PaneRow>` (tmux order; `active` = pid == client pid), `pane_pass(wanted, just_enabled, due) -> PanePass { Skip, Derive }`, `PaneMeta { pane_pid, active, agent, log }`, `TabMeta` (+ `panes`, `log`), `Agent`, `AgentState`, `AgentKind`; `TabMetaHandle::set_panes_wanted(&self, bool)`.
- Turning panes on runs one full pass over every tab; turning them off clears published `panes`. The active pane copies the tab's `agent` and `log`.

### T2 — dock seam (`crates/app/src/{seam,overlay,terminal_app,tabs}.rs`)
- Crate-root exports: `RightDock { None, Monitor, Flow }` (`Default` = `None`) with `toggle(self, Occupant)`, `restored(bool)`, `persisted_open(self)`; `Occupant { Monitor, Flow }`; `AppRequest::{ToggleDock(Occupant), ActivateTab(TabId)}` (`SetMonitorOpen` removed); `OverlayCommand::DockChanged { dock, cwd: Option<PathBuf>, shell_pid: Option<u32> }` (`RestoreMonitor` removed), fanned out like `Cancel`; `AppCtx::{toggle_dock(Occupant), activate_tab(TabId)}`, `AppCtx::new(..)` and `take_requests()`; `Overlay::on_press(&mut self, [f32; 2], &mut AppCtx)`; `OverlayHost::press(&mut self, [f32; 2], &mut AppCtx)`; `tab_position(&[TabId], TabId) -> Option<usize>`; `Tabs::position`; `AiMonitorOverlay::with_waker(None, no_deadline())`, `Handled`, `KeyInput` (public fields), `OverlayHost`.
- `Tabs::new` needs the crate-private `WakeSource`, so integration tests use `tab_position`, not a live `Tabs`.
- The monitor's `on_command(DockChanged)` returns `Passthrough` and is edge-triggered.

### T4 — flow overlay (`crates/app/src/{flow,flow_telemetry}.rs`)
- `FlowOverlay` registered in `feature_overlays` at priority 99 (directly after the ai-monitor's 100); `wiring.rs` counts 6 overlays (7 with the metadata pump).
- Unsplit tab fold key is `(TabId, 0)` — no published field carries the tab's root pid (D8).
- Subagent backfill is a line-by-line fold (not `ai_monitor::backfill_telemetry`) so the stop flag is checked between lines (R7.3); tailing polls file length every 250 ms (not `LogWatcher`, which is per-project-dir); a shrinking file restarts its fold; an unterminated last line waits for its newline.
- The change key is checked in `poll()` and once per frame in a draw-layer wrapper, because a blocking-dialog row change wakes no overlay.
- Labels: `window`, `pane {pid}`, `claude`. Cmd+Shift+F rejects Ctrl/Alt.
