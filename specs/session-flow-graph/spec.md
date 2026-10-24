# Spec: session-flow-graph

## Meta
- **Created**: 2026-10-19
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-10-20
- **Revises**: `specs/session-flow-viewer/spec.md` (approved 2026-10-18, implemented
  uncommitted on `feat/session-flow`). Decisions of that spec not named as superseded
  below stay in force; this spec names each one it replaces.
- **Reference design**: a left→right card diagram (OpenClaw "Memory Architecture":
  rounded cards in columns joined by curved arrows), supplied by the user 2026-10-19.

## Goal
Revise the session flow viewer into a card graph: "this kind of UX, with the existing
session card UI as the card image — the main project on top, sessions created in
worktrees connected under it, and if that main session runs a sub-session, show that
too." Chosen by the user on 2026-10-19: a full-window overlay; hierarchy project →
worktree → subagent; "sub session" means the Agent tool's subagents; revise the spec,
then implement.

## Non-goals
- Window, tab and tmux-pane nodes (the tree's old levels).
- Claude sessions running outside this terminal.
- Other "sub sessions" than Agent-tool subagents (e.g. another `claude` in the same
  directory is a sibling card, not a child).
- Reflowing the terminal for the viewer, or sharing the right dock with the ai-monitor.
- Multiple windows: the app has one window (`terminal_app.rs:161`); the graph shows that
  window's sessions.
- Free-form canvas interaction: drag, zoom, pan, or user-arranged cards.
- Animation of edges or cards.
- Restyling the sidebar card itself; the graph reuses it as drawn.

## Confirmed Goal
`Cmd+Shift+F` opens a **full-window session graph** drawn over the terminal (no
reflow). `Esc` or `Cmd+Shift+F` closes it. Nodes are the **existing sidebar session
cards** (`sidebar_card::build_card`), laid out in left→right columns and joined by
curved connector lines:

1. **Project column** — the card of each Claude session running in a git repository's
   **main worktree**.
2. **Worktree column** — the card of each Claude session running in a **linked
   worktree** of that same repository, connected to the project card.
3. **Subagents** — each session's Agent-tool subagents, connected to that session's
   card; running ones are drawn live, finished ones greyed and folded behind an
   `N finished` node.

- A header reads `{w} working · {q} waiting / {n}`; each session card's state always
  equals its tab's sidebar card (one classifier).
- Clicking a session or subagent card switches to that session's tab and closes the
  graph.

**Done when** the user runs Claude in a repository's main worktree and in one of its
linked worktrees, opens the graph and sees the worktree card connected under the
project card beside the main card; then, with the graph left open while a background
subagent or a prompt sent just before opening runs to completion, watches the header's
working count change live without reopening it.

Inferences beyond the literal request (confirmed by the user 2026-10-19):
- (inferred) A repository with worktree sessions but **no main-worktree session** gets
  a status-less **project placeholder card** (the repository's name) as its root, so
  the group survives; it is not counted in the header.
- (inferred) Several Claude sessions in the same directory — two tabs, or two tmux
  panes — are separate cards in the same column. Panes are not nodes, but each pane's
  session is still its own card.
- (inferred) Tabs with no Claude session do not appear.
- (inferred) A session outside any git repository is its own root.
- (inferred) While open, the overlay consumes keys (acting only on `Esc` and
  `Cmd+Shift+F`) and covers the sidebar too; more cards than fit scroll vertically by
  wheel.
- (inferred) The right dock returns to being the ai-monitor's alone; the graph covers
  an open monitor rather than replacing it.

## Research

### What exists from session-flow-viewer (uncommitted, `feat/session-flow`)
- Per-pane derivation in the tabmeta worker: `PaneMeta { pane_pid, active, agent, log }`
  (`crates/app/src/tabmeta/mod.rs:120`), panes enumerated only while wanted
  (`TabMetaHandle::set_panes_wanted`, `PanePass` `mod.rs:129`).
- Subagents per log: viewer telemetry thread `crates/app/src/flow_telemetry.rs` (510
  lines), fed from `ai_monitor::telemetry::Subagent`.
- Status join: `row_meta` / `resolve_status` made `pub` (`crates/app/src/sidebar_model.rs`),
  used by `build_flow(rows, metas, home, subagents, folds)` (`crates/app/src/flow.rs:160-240`),
  which emits a flat indented `FlowModel { rows }` (`crates/renderer/src/flow_layout.rs:127`)
  with `header()` at `flow_layout.rs:140`.
- Renderer panel: `flow_layout.rs` (970 lines) + `flow_panel.rs` (1088) — rung-index
  list in the right-dock band, `FlowHandle`, `FlowHit`, press → `FlowTarget::{Node, Fold}`.
- Dock seam: `AppRequest::{ToggleDock(Occupant), ActivateTab(TabId)}` (`crates/app/src/seam.rs:159,165`),
  `RightDock` (`seam.rs:191`), `Running::toggle_dock` (`crates/app/src/terminal_app.rs:1466`),
  `OverlayCommand::DockChanged`; `on_press(point, &mut AppCtx)` (`terminal_app.rs:914`).
- Presses reach overlays only inside the right band: `input::in_monitor_panel`
  (`crates/app/src/input.rs:757`, call sites `terminal_app.rs:2736,2949`).
- Acceptance tests TT1–TT9 (`crates/app/tests/tt{1..8}_*.rs`, `crates/renderer/tests/tt{8,9}_*.rs`),
  hashed in `specs/session-flow-viewer/tt-hashes.txt`.

### The card to reuse
- `sidebar_card::build_card(&CardInput) -> CardDraw { shapes, runs, title_zone }`
  (`crates/renderer/src/sidebar_card.rs:295`, input struct `:70`): pure, takes a rect,
  `AgentSkin`, `CardState`, `CardStatus`, title / folder / branch strings and the face
  metrics; returns shapes + text runs per `CardFace`.
- Its only caller is the sidebar layer `TabBar::record` (`crates/renderer/src/sidebar_row.rs:1531-1600`),
  which owns `CardAtlases` (`sidebar_text.rs:87`, 17px title / 12.5px meta, `ensure`
  per device + scale) and three `SidebarTextPipeline`s (title, meta, fallback) plus a
  `SidebarShapePipeline` (`sidebar_shapes.rs:346`). Card height comes from
  `SidebarLayout.card_h` (`sidebar_row.rs:265,283`), not from the builder.
- The card's text comes from `RowMeta { branch, status, skin, line1, cwd_display }`
  (`sidebar_row.rs:114`), produced by the app's `row_meta`.

### Drawing connectors
- `SidebarShape` offers `rounded_box`, `capsule(a, b, r)` and `ring` SDF primitives
  (`crates/renderer/src/sidebar_shapes.rs:146,179,217`); there is no curve primitive.
  A curve can be drawn as a short chain of capsules along a sampled cubic Bézier.

### Repository / worktree identity
- `tabmeta/git.rs` walks ≤ 40 ancestors to the first `.git` (`resolve_head_path`,
  `crates/app/src/tabmeta/git.rs:170`): a directory is a main worktree; a file holding
  `gitdir: <path>` (`parse_gitdir_file`, `git.rs:233`) is a linked worktree or a
  submodule. Only HEAD is read today; the repository's **common dir** (the linked
  worktree's `<gitdir>/commondir`) is not resolved anywhere. Memoised by cwd with
  revalidation (`git.rs` module doc).
- The claude process's own cwd is readable as `proc::cwd_of_pid`
  (`crates/app/src/tabmeta/proc.rs:26`); `TabMeta.cwd` is the tab's (or active pane's)
  **shell** cwd, which can differ after a `cd` (architecture G5).

### Overlay and key plumbing
- Overlays see keys before sidebar/tab chords (`terminal_app.rs` key path); `on_key`
  returns `Handled` (`seam.rs:341`). Layer z-order: glyph 0 < tab bar 10 < image 20 <
  overlay 100 (`crates/renderer/src/lib.rs:176`). `FrameCtx` carries the full `viewport`
  and the insets (`lib.rs:136-158`).

## L2 Checkpoints

Complexity: **Complex (4 signals)** — external resources (session `.jsonl` logs, git
metadata files, `tmux`); 3+ subsystems (tabmeta worker, telemetry thread, renderer
card/edge drawing, input routing); async (worker + telemetry thread); multi-stage state
(overlay open/closed, subagent running → finished, fold open/closed).
Brownfield weights: Core 30 · Scope 20 · Error 25 · Data 15 · Implementation 10.

| Dimension | Checkpoints | Resolved by |
|-----------|-------------|-------------|
| Core | graph shape and columns; root when a repo has 0 / 1 / ≥2 main sessions; card content per node kind; status source; header; open/close keys; click targets; subagent display; refresh | user 2026-10-19 (D1, D2, D4, D5, D7, D8), carried (old D2, old D3), resolved + carried (D3), carried (D11), assumed (D6) |
| Scope | which sessions; non-git sessions; tabs without Claude; panes; ai-monitor relationship; multi-window | user (D1, D4, Non-goals), carried (old D5, old D9) |
| Error/Edge | git unreadable / submodule / bare; claude cwd differs from shell cwd; repo identity changes while open (worktree removed); too many cards (overflow); window too narrow for three columns; offscreen parent edges; covered overlays still taking input; telemetry and tmux failures | assumed (D5, D10, D12, D13, D16), carried (old D14, old D15, old D18) |
| Data | repo identity publication; model shape (nodes, columns, edges); cache key | assumed (D10, D11, D15) |
| Implementation | card drawing reuse (atlases, pipelines); curve primitive; dock-seam revert; placement; tests | L1 (D14), assumed (D4 revert, D9, D15, D17) |

Fenced checkpoints: none. Composite 1.00 counting `assumed` as resolved (2 user answers
this round, the rest carried or assumed). The assumptions with the most effect, put to
the user at the L2 gate: **D10** (grouping by the claude process's cwd), **D12/D13**
(slot layout, narrow fallback, scroll culling) and the glance-while-typing trade-off of
**D4/D5** (Known Gaps). Inversion probe: *every requirement met yet it fails* if a
session is grouped under the wrong project — e.g. the shell `cd`s into another repo while
`claude` keeps running in the first. Grouping therefore keys on the **claude process's**
cwd, not the shell's (D10). Implication probe: "the overlay covers the whole window"
also means presses must reach it anywhere on screen, not only inside the right band
(D6).

## Decisions

Carried unchanged from `specs/session-flow-viewer/spec.md`: **D2** (status is the
card's, through `row_meta` / `resolve_status`), **D3** (header format and what it
counts; project cards and subagents are never counted), **D5** (scope: Claude sessions
inside this terminal only), **D9** (tmux panes enumerated by the worker only while the
viewer is open, active pane chosen by pid), **D10** (the viewer-owned subagent telemetry
thread and its lifecycle), **D13** (non-active pane sessions are log-only), **D14**
(degradation without tmux or logs), **D15** (subagent cap 32), **D18** (remaining
failure edges, except its tab-node bullet, superseded by D1 here). Superseded: **D1**
(tree shape), **D4** (shared right dock), **D6** (the chord's dock semantics), **D7**
(click targets), **D8**'s presentation (its fold semantics stay), **D11**'s key (scroll
unit), **D12** (row-ladder overflow), **D16** (placement), **D17** (verification shape).
In this spec they are numbered afresh; "old Dn" means the revised spec's decision.

### D1: Graph shape — project → session → subagent, in three columns
- **Status**: resolved (user 2026-10-19)
- **Rationale**: Column 0 holds one **project card** per git repository that has at least
  one Claude session in this terminal. Column 1 holds that repository's **session
  cards**: first the sessions in its main worktree, then the sessions in its linked
  worktrees, each group in tab-strip order (and, inside a split tab, pane order). Every
  session card is connected to its project card. Column 2 holds each session's
  **subagent** nodes (D8), connected to that session. A session outside any repository
  (D10) sits in column 1 with no parent and no edge; all of them come after every
  repository group. Repository groups are ordered by the strip position of their first
  session. Tabs without a Claude session, windows and panes are not nodes (panes still
  yield one card per pane session, via old D9). Rejected: the main-worktree **session**
  as root (ambiguous with two main sessions, and absent with none), connecting worktrees
  to the first main session (tab order would decide hierarchy), and a free 2D layout
  (no requirement needs it; columns make hit-testing and scrolling plain).

### D2: The project card
- **Status**: resolved (user) + assumed (content)
- **Rationale**: Drawn with the same `build_card` as a session card, with `status:
  Hidden` (no status line), `AgentSkin::Generic`, `title` = the main worktree
  directory's basename, `folder` = that directory `~`-abbreviated, `branch` = none. It is
  always present for a repository group — including when no session runs in the main
  worktree — and is never counted (old D3). Clicking it does nothing (D7). Rejected: a
  smaller header chip (the user chose one card image for every node) and showing the
  main worktree's branch (it can differ from every session's, and reads as a session).

### D3: Session cards are the sidebar's card, with the sidebar's content
- **Status**: resolved (user) + carried (old D2)
- **Rationale**: A session card is `build_card` fed exactly the `RowMeta` the sidebar
  card gets for that tab — `line1`, `cwd_display`, `branch`, `skin`, `status` — from
  `row_meta(row, meta, home)`. A non-active pane session takes the tab's `line1`, its
  **own agent's** skin, its own status per old D13, and the folder and branch of its own
  cwd. (Amended 2026-10-21, user decision: a Claude pane always looks like Claude, even
  when the tab's active pane is a shell.) `state`
  is `Selected` for the active tab's session (its active pane, when split) and `Default`
  otherwise; `Hover` while the pointer is over the card. `show_menu` and `editing` are
  always false (no menu or rename in the graph). Rejected: a graph-specific card (the
  user asked for the existing card UI).

### D4: Placement — a full-window overlay, the right dock reverts to the monitor alone
- **Status**: resolved (user 2026-10-19) + assumed (revert)
- **Rationale**: While open, the graph covers the whole viewport — sidebar, tab bar and
  terminal — with an opaque backdrop (`SidebarTheme.panel_bg`) at z **110**, above the
  ai-monitor panel (100), so the monitor's band never shows through. No
  inset changes and the grid never reflows. The ai-monitor keeps its band and its own
  open state underneath; the graph simply covers it. Consequently the session-flow-viewer
  dock seam is **reverted**: `RightDock`, `Occupant`, `AppRequest::ToggleDock`,
  `OverlayCommand::DockChanged` and `Running::toggle_dock` are removed, and the monitor's
  toggle, request and persistence return to their shape on `main`
  (`AppRequest::SetMonitorOpen`, `OverlayCommand::RestoreMonitor`). Kept from that seam:
  `AppRequest::ActivateTab`, `on_press(point, &mut AppCtx)` and `tab_position`. The
  graph's open state is never persisted; the app always starts with it closed.
  Rejected: keeping `RightDock` with one occupant (an abstraction with nothing left to
  arbitrate) and a right-dock graph. The strongest case for the dock is the
  glance-while-typing use: the header is for watching work change, and a covering
  overlay must be closed to type. It is answered by the sidebar, which already shows
  every tab's live state while typing; the graph is for the structure (which sessions
  belong to which repo and worktree, and what each has spawned), which three columns of
  cards cannot show in ~400 px. The user chose the full window knowing this (2026-10-19);
  a pass-through or docked mode is a follow-up (Known Gaps).

### D5: Open, close and keys
- **Status**: resolved (user) + assumed (key policy)
- **Rationale**: `Cmd+Shift+F` toggles the graph. The graph overlay has priority **300**,
  above rename (200) and the ai-monitor (100), so it sees every key first. An open rename field or card menu is left as it is: it
  receives nothing while covered (keys and presses stop at the graph) and is still
  there when the graph closes — including the menu that was open when `Cmd+Shift+F` was
  pressed: a key consumed while the graph is open before or after it does not trigger the
  card menu's "a claimed key dismisses the menu" rule, and `Esc` under the graph is the
  graph's close key, not the menu's dismissal (amended 2026-10-21). (Fanning out `Cancel` was rejected: overlays cannot issue
  `OverlayCommand`s — only the app calls `OverlayHost::command` — and a new request for
  it would deliver `Cancel` to the graph itself.) While it is open the overlay consumes
  **every** key: `Esc` and `Cmd+Shift+F` close it, every other key is swallowed (never
  reaches the PTY, the sidebar or the tab chords). `Cmd+Q` is not a key event the overlay
  sees and still quits. The graph also closes when a click activates a tab (D7). It does
  **not** close when the active tab changes by other means (a tab closing under it). The
  wheel scrolls it (D13). Rejected: passing keys through to the terminal (typing into a
  covered screen is invisible input) and closing on any click (a fold click must keep it
  open).

### D6: Presses anywhere reach the open overlay
- **Status**: assumed (implication of D4)
- **Rationale**: Today presses reach overlays only inside the right band
  (`input::in_monitor_panel`). While the graph is visible, every left press in the
  window is routed to it first, before the sidebar, monitor and grid handlers, and a
  press it does not resolve to a node is consumed (no selection starts under it). The
  **wheel** is routed the same way: anywhere in the window it scrolls the graph (D13) and
  never the monitor, the scrollback or the sidebar. Hover tracking for D3's `Hover`
  state uses the cursor-moved path the same way. When closed, routing is
  exactly today's. Rejected: hit-testing only the card rects and letting other presses
  fall through (they would act on a terminal the user cannot see).

### D7: Click targets
- **Status**: resolved (user: "clicking a card jumps to its tab") + assumed (details)
- **Rationale**: A session card or a subagent card → `AppRequest::ActivateTab(tab)` and
  the graph closes. In a split tab the tab is activated; tmux pane focus is not attempted
  (as before). The `N finished` node toggles that session's fold and the graph stays
  open. The project card, edges, header and empty space do nothing.

### D8: Subagents — the same card, finished ones greyed behind `N finished`
- **Status**: resolved (user 2026-10-19) + carried (old D8 semantics)
- **Rationale**: A running subagent is a `build_card` with `title` = `subagent_type`,
  else `description`, else `agent`; `folder` = its `description` (empty when it was the
  title); `branch` none; `skin` = the parent session's; `status: Working`. When a session
  has finished subagents, one **fold node** follows its running ones in column 2: a
  `SidebarShape::capsule` chip (not a card — it is a control) reading `▸ N finished` /
  `▾ N finished`. Expanded, the finished subagents follow it as cards with `status:
  Hidden` drawn at reduced opacity (the muted look). Fold state is keyed by session
  identity `(TabId, pane_pid)` and pruned on rebuild, exactly as old D8. Rejected: a chip
  for every subagent (the user chose the card), hiding finished ones (old D8).

### D9: Edges
- **Status**: assumed
- **Rationale**: Each parent→child edge is a cubic Bézier from the parent card's
  right-edge midpoint to the child's left-edge midpoint, control points offset
  horizontally by half the column gap — the reference design's S-curve. It is drawn as
  `EDGE_SEGMENTS` (16) `SidebarShape::capsule`s along the sampled curve, stroke 1.5 px ×
  scale, colour `SidebarTheme.card_border_active` at `EDGE_ALPHA` = **0.45**, with a
  `ring` dot of radius 3 px × scale at the child end. An edge to a finished (muted) child
  is drawn at `EDGE_ALPHA × MUTED_ALPHA`. (Amended 2026-10-21, user decision: the
  original `CARD_BORDER_ALPHA` (0.10) put an edge ~6 sRGB counts above `panel_bg`, barely
  visible; 0.45 puts it ~22 counts above.) Edges are drawn before cards, so cards cover their ends. Rejected: a new
  line/curve shader (the capsule SDF already antialiases; 16 segments per edge is a few
  hundred instances at most) and straight elbows (the user pointed at curves).

### D10: Repository identity — from the claude process's cwd, published by the worker
- **Status**: assumed
- **Rationale**: For each Claude session (a tab's agent, or a pane's) the worker reads
  the claude process's cwd with `proc::cwd_of_pid(agent_pid)`, falling back to the tab's
  or pane's shell cwd when the pid is unknown, and resolves it in `tabmeta/git.rs`:
  - walk to the first `.git` (≤ 40 ancestors, as today);
  - `.git` is a **directory** → main worktree; `common` = that `.git`; `main_root` = its
    parent;
  - `.git` is a **file** `gitdir: <g>` and `<g>/commondir` exists → linked worktree;
    `common` = `<g>/<commondir contents>` normalised; `main_root` = `common`'s parent;
  - `gitdir:` with no `commondir` (a submodule) → treated as its own main worktree, with
    `common` = `<g>` and `main_root` = the directory holding the `.git` file;
  - no `.git`, the cwd itself no longer existing, or any read failing → no repository.
    (The existence check matters: linked worktrees live both beside the main repository
    and **inside** it — measured 2026-10-20, `git worktree list` here shows
    `terminal-feat-session-flow` beside `terminal` and `terminal/.claude/worktrees/agent-*`
    inside it — and a removed inside one's ancestor walk would otherwise land on the main
    `.git` and regroup the session as a main one.)
  Published as `repo: Option<RepoId { common: PathBuf, main_root: PathBuf, linked: bool }>`
  on `TabMeta` (the tab's session) and on `PaneMeta` (each pane's), plus `PaneMeta.cwd` and `PaneMeta.branch`
  (the git memo's branch for that cwd) for D3's folder and branch lines. Resolved only while the graph is open (it rides old D9's
  `set_panes_wanted` switch, renamed `set_graph_wanted`, including its immediate pass
  over every tab on open), memoised by cwd alongside the branch memo. A resolved entry
  is **revalidated every tick** by one `stat` of the `.git` file or directory it was
  found from, plus, for a linked worktree, one `stat` of `<g>/commondir` — the same warm-path shape as the branch memo's HEAD `stat` — so a deleted
  worktree directory (`git worktree remove`, or `rm -rf` without `prune`) misses on the
  next tick and re-resolves cold, where the cwd-existence check applies. A no-repository
  entry expires after the existing `NEG_MEMO_TTL_TICKS` (≈ 10 s), so a later `git init`
  is noticed.
  **Readiness.** `repo: None` means "no repository", never "not resolved yet": the
  worker publishes a `graph_ready: bool` on `TabMetaShared`, set by the first publish
  after `set_graph_wanted(true)` whose pass covered every tab, and cleared — together
  with every published `repo`, `PaneMeta` and pane list — on `set_graph_wanted(false)`.
  A reopen therefore starts from `graph_ready == false`, never from the previous open's
  groups. Because the commands are queued, a reopen can `sync` before the worker has
  reached its close or open and copy the previous open's readiness, so readiness carries
  an **epoch** (amended 2026-10-21): `set_graph_wanted(true)` returns a fresh epoch and
  sends it in the command, the worker publishes it beside `graph_ready`
  (`TabMetaSnapshot::graph_epoch`), and the overlay treats a snapshot as ready only under
  its own open's epoch — until then it reads `resolving…`.
  Sessions group by `common`; the project card shows `main_root`. Rejected: grouping by
  the shell cwd (a `cd` would regroup a session that never moved, G5), running `git
  rev-parse` (a process per session per tick) and grouping by directory prefix (linked
  worktrees usually live outside the main directory).

### D11: Refresh — rebuild only on change
- **Status**: carried (old D11) with a new key
- **Rationale**: The overlay rebuilds the `FlowGraph` model only when `(tabmeta
  generation, rows_rev, subagent generation, fold set, active tab)` changes; the renderer
  lays it out only when the model, viewport, scale or atlas identity changes, and
  re-composes only when the scroll offset or hover changes. Unchanged key → the cached
  draw lists are resubmitted (G1).

### D12: Layout geometry
- **Status**: assumed
- **Rationale**: Every node occupies one **slot**: a fixed-height row of the sidebar's
  card height (`CardMetrics::DEFAULT.scaled(scale).height`) plus a row gap. A subagent
  or fold node takes one slot; a session takes `max(1, its column-2 nodes)` slots and its
  card is drawn in the first; a project takes the sum of its sessions' slots and its card
  is drawn in its first slot (top-aligned, so a node never depends on rows below it). The
  card width is the sidebar card's width (`[sidebar].width_px` minus 2 × `CardMetrics.inset_x`, scaled), shrunk to fit three columns plus two column
  gaps and margins in the viewport, never below `MIN_CARD_W` (180 px × scale). When three
  columns do not fit at `MIN_CARD_W`, column 2 is not drawn: a session with subagents
  takes exactly **one** slot (a project still takes the
  sum of its sessions' slots) and shows a `+N` capsule chip at its card's right edge, `N` = its **running** subagents
  plus its finished ones, and the chip is inert (folds are unreachable in this mode).
  When even two columns do not fit, column 0 is dropped too and sessions are drawn
  alone in one column, still in group order. When one card does not fit, nothing but
  the header is drawn. The header line sits above the slots and does not scroll. Cards'
  own text truncation (`build_card`) handles long titles.

### D13: Overflow — scroll by slot, no scissor
- **Status**: carried (old D12's principle) with the slot as the unit
- **Rationale**: When the slots are taller than the viewport below the header, the wheel
  scrolls by whole slots, clamped so the last slot stays reachable; the offset lives
  outside the model. A node whose slot is above the offset or below the viewport is not
  drawn; an edge is drawn only when **both** of its ends are drawn, so no line runs off
  the top without an end. The project card of a group scrolled partly past is therefore
  hidden with its edges until scrolled back — accepted for the no-scissor rule.
  Rejected: pixel scrolling (needs a scissor the shape path does not have) and a sticky
  project card (it would draw over the slot rows beneath it).

### D14: Card text drawing — the panel's own card atlases
- **Status**: resolved (L1) + assumed
- **Rationale**: The graph layer owns its own `CardAtlases` (title 17 px, meta 12.5 px)
  and the same pipeline set the sidebar uses (`SidebarShapePipeline` for shapes,
  `SidebarTextPipeline` per face plus the grid-atlas fallback), resolving `build_card`'s
  runs exactly as `TabBar::record` does; the resolution code is factored out of
  `sidebar_row.rs` into a shared function rather than copied. The atlases are built on
  first open, not at startup. Rejected: borrowing the sidebar's atlases (they are private
  state of another layer, and a second borrower would couple the two layers' eviction).

### D15: Code placement
- **Status**: assumed
- **Rationale**: Renderer: new `graph_layout.rs` is the pure graph layout (`FlowGraph` →
  slots, card rects, edge capsules, fold chips, hit boxes); new `graph_panel.rs` is the
  full-window layer (T4 deletes the old `flow_layout.rs` / `flow_panel.rs` once nothing uses them) (`FlowHandle` with `set`, `set_visible`, `press`, `hover`, `wheel`).
  `FlowGraph` carries plain data only: per node a `kind` (`Project`, `Session`,
  `Subagent`, `Fold`), `parent: Option<usize>`, the card strings and `CardStatus` /
  `AgentSkin`, `muted`, `selected`, and a `FlowTarget` (`Node(u64)` / `Fold(u64, u32)` /
  `None`) — no app types. App: `flow.rs` `build_graph(rows, metas, home, subagents,
  folds) -> (FlowGraph, FoldSet)`; `tabmeta/git.rs` gains `repo_of(cwd)`; the dock seam
  revert touches `seam.rs`, `overlay.rs`, `terminal_app.rs`, `rename.rs`, `lib.rs`.
  `flow_telemetry.rs` and the tmux pane code are unchanged except T2's renames
  (`set_graph_wanted`, `graph_pass`) and D10's identity publication.

### D16: Remaining edges
- **Status**: assumed
- **Rationale**: A worktree removed under a live session (`git worktree remove`) → its
  cwd no longer exists, or its `commondir` read fails → no repository (D10) → the
  session moves to the ungrouped tail on the next tick (D10's per-tick `.git` `stat`). Two repos with the same directory basename → two project cards with the
  same title, distinguished by their folder line. A bare repository has no worktree a
  session could run in, so it never appears. A tab whose session resolves to a repo after
  the graph opened regroups within the no-repository TTL (≈ 10 s, D10).
  **First frame after opening:** until the worker publishes its first pass with the graph
  wanted — `graph_ready` (D10), ≤ one tick via old D9's immediate pass — the header
  reads `resolving…` and no cards are drawn, on every open, so sessions never appear ungrouped and then jump into groups.

### D17: Verification shape
- **Status**: assumed
- **Rationale**: Pure functions — `repo_of`, `build_graph` (grouping, order, folds,
  pruning), the slot layout, edge sampling, hit-testing, scroll clamping, key policy —
  get unit and acceptance tests. The layer gets a golden frame through
  `Renderer::capture` asserting card ink in each column, an edge's pixels between two
  cards, the three status colours, and nothing of the terminal showing through the
  backdrop — including no ai-monitor ink with the monitor open beneath. Of the session-flow-viewer TT files only old TT6 (tmux panes, minus its
  R6.5 field lock) and old TT7 (telemetry) stand; the rest encode the dock or the tree
  and are replaced by this spec's TTs. A new `tt-hashes.txt` is taken when the TT set is
  final. `cargo mutants -f` on every changed source file, 0 non-equivalent survivors.
  `wiring.rs` keeps 6 overlays.

## Constraints
- The renderer never imports app or winit types; `term-core` is untouched (dependency-direction).
- No GUI crates; cards and edges use the existing SDF shape pipeline and glyph atlases (no-gui-crates).
- Nothing new runs while the graph is closed: no pane enumeration, no repo resolution, no telemetry thread, no card atlases rebuilt (G1).
- No per-frame model rebuild or relayout on an unchanged key (G1).
- A session card's status comes only from `row_meta` / `resolve_status` (one classifier).
- The graph reads; it never mutates tmux, git or any session.
- While open, no key or press reaches the terminal, sidebar or tab chords except through the graph's own handling.
- No `unwrap()` outside tests; macOS only; local zsh only.

## Known Gaps
- Below `MIN_CARD_W` the subagent column is dropped for a `+N` chip (D12); a very narrow window shows less.
- A project card scrolled partly past its group is hidden with its edges until scrolled back (D13).
- Foreground subagents appear only on return (G8) and non-active pane sessions are log-only (old D13), as before.
- A subagent card has no folder/branch of its own; its second line is its description.
- Submodules are treated as their own repositories (D10), not as children of their superproject.
- `main_root` = parent of `common` is wrong for a submodule's linked worktree (`common` =
  `.git/modules/x`) and for a `--separate-git-dir` repository: their project card shows a
  wrong folder line, though grouping by `common` is still right.
- **Glance while typing** (D4): the graph covers the terminal, so watching it means not
  typing; the sidebar remains the live surface. A docked or pass-through mode is the
  follow-up if this proves annoying.

## Requirements

Old R3 (status), R6 (tmux panes) and R7 (telemetry thread) of
`specs/session-flow-viewer/spec.md` carry over with the API changes R4 and R13 state;
their GWTs are restated here where the graph changes their When.

### R0: The graph shows every session grouped by repository, live — D1, D3, D4

#### R0.1: Worktree card under its project, live count (manual)
- **Given**: the app runs with tab 1 running `claude` in a repository's main worktree and tab 2 running `claude` in one of its linked worktrees; a prompt has just been sent in tab 2
- **When**: the user presses `Cmd+Shift+F` and waits for the prompt to finish without closing the graph
- **Then**: one project card titled with the repository's directory name is connected by curved edges to two session cards — tab 1's above tab 2's — each identical in title, folder, branch and status to its sidebar card; the header's `working` count drops from 1 to 0 as tab 2's card turns from Working, with the graph still open

### R1: Open, close and key ownership — D4, D5

#### R1.1: The chord toggles the graph
- **Given**: a `FlowOverlay` (closed)
- **When**: `Overlay::on_key` receives `Cmd+Shift+F` twice
- **Then**: both return `Consumed` and queue no `AppRequest`; `visible()` is true after the first and false after the second

#### R1.2: Esc closes, every other key is swallowed while open
- **Given**: an open `FlowOverlay`
- **When**: `on_key` receives `a`, `Enter`, `Cmd+T`, `Cmd+Shift+M`, then `Esc`
- **Then**: each returns `Consumed` and queues no `AppRequest`; `visible()` stays true until `Esc`, after which it is false

#### R1.3: A closed graph passes every other key through
- **Given**: a closed `FlowOverlay`
- **When**: `on_key` receives `a`, `Esc` and `Cmd+Shift+M`
- **Then**: each returns `Passthrough` and `visible()` stays false

#### R1.4: The graph sees keys before every other overlay
- **Given**: `app::feature_overlays(..)`
- **When**: the registered overlays' priorities are read
- **Then**: the `FlowOverlay`'s priority is 300 and strictly greater than every other overlay's (rename 200, ai-monitor 100)

#### R1.5: The graph is never restored open
- **Given**: a config whose persisted state has the ai-monitor open
- **When**: the app restores overlay state at start (`OverlayCommand::RestoreMonitor` fan-out)
- **Then**: the ai-monitor becomes visible and the `FlowOverlay` stays closed

### R2: Pointer routing while the graph is open — D6

#### R2.1: The open graph owns every press, wheel and hover
- **Given**: the pure router `app::pointer_owner(graph_visible: bool, in_monitor_band: bool, in_sidebar: bool) -> PointerOwner` (`PointerOwner { Graph, Monitor, Sidebar, Grid }`)
- **When**: it is called with `graph_visible = true` and all four combinations of the other two flags
- **Then**: it returns `Graph` every time

#### R2.2: A closed graph leaves today's routing
- **Given**: the same router
- **When**: it is called with `graph_visible = false` and each combination of `in_monitor_band` / `in_sidebar`
- **Then**: `in_monitor_band` → `Monitor`, else `in_sidebar` → `Sidebar`, else `Grid` — the precedence `terminal_app.rs` applies today

#### R2.3: An unresolved press is consumed
- **Given**: an open `FlowOverlay` whose handle published hit boxes, and a point inside none of them
- **When**: `Overlay::on_press(point, ctx)` is called
- **Then**: it returns `Consumed`, queues no request, and the graph stays open

### R3: The right dock is the ai-monitor's alone again — D4

#### R3.1: The monitor's chord toggles the monitor only
- **Given**: an `AiMonitorOverlay` (closed) and a closed `FlowOverlay`
- **When**: `Cmd+Shift+M` is dispatched through `OverlayHost::dispatch`
- **Then**: the context holds exactly `AppRequest::SetMonitorOpen(true)` and the `FlowOverlay` is still closed; dispatched again, `SetMonitorOpen(false)`

#### R3.2: Opening the graph leaves the monitor's state alone
- **Given**: an open ai-monitor
- **When**: `Cmd+Shift+F` opens the graph and `Esc` closes it
- **Then**: no `SetMonitorOpen` request was queued and the monitor's `visible()` is true throughout

#### R3.3: The graph does not reserve the band
- **Given**: a headless `Renderer` with the monitor closed and a `FlowPanel` layer whose handle is visible
- **When**: a frame records
- **Then**: `Renderer::right_inset()` is 0 and `grid_size()` equals its value with no flow layer

### R4: Repository identity — D10, D16

#### R4.1: A main worktree
- **Given**: a temp directory `r` with a `.git` directory, and a cwd `r/src/x`
- **When**: `app::repo_of(cwd)` is called
- **Then**: it returns `Some(RepoId { common: r/.git, main_root: r, linked: false })`

#### R4.2: A linked worktree, beside or inside the main one
- **Given**: `r/.git/worktrees/w1/commondir` containing `../..`, and two worktree directories whose `.git` file reads `gitdir: <r>/.git/worktrees/w1` — one at `r-w1` (beside), one at `r/.claude/worktrees/w1` (inside)
- **When**: `app::repo_of` is called on each worktree directory
- **Then**: both return `Some(RepoId { common: r/.git, main_root: r, linked: true })` (paths normalised, no `..` left)

#### R4.3: A submodule is its own repository
- **Given**: a directory `s` whose `.git` file reads `gitdir: ../.git/modules/s` and whose target holds no `commondir`
- **When**: `app::repo_of(s)` is called
- **Then**: it returns `Some(RepoId { common: <parent>/.git/modules/s, main_root: s, linked: false })`

#### R4.4: No repository
- **Given**: a cwd with no `.git` ancestor within 40 levels; a cwd that does not exist; a `.git` file with no `gitdir:` line; a linked worktree whose `commondir` is unreadable
- **When**: `app::repo_of` is called on each
- **Then**: each returns `None`

#### R4.5: The memo revalidates by `stat` and notices removal
- **Given**: the repo memo with a resolved linked-worktree entry
- **When**: the next lookup runs with nothing changed, and then again after the worktree directory is deleted
- **Then**: the first lookup costs at most two `stat`s and no reads (the `probe` counters); the second returns `None`

#### R4.6: The worker publishes identity only while the graph is wanted
- **Given**: the `TabMeta` and `PaneMeta` structs and `TabMetaShared`
- **When**: the field-lock tests enumerate their fields
- **Then**: `TabMeta` has exactly `branch, agent, cwd, task, agent_pid, panes, log, repo, graph_agent, graph_task` (the last two added by the amendment "Graph ignores `[sidebar].show_agent_status` (2026-10-22)"); `PaneMeta` has exactly `pane_pid, active, agent, log, cwd, branch, repo`; and `TabMetaShared` carries `graph_ready`

#### R4.7: Readiness is set by a full pass and cleared on close
- **Given**: the worker's pure gate `app::graph_pass(wanted: bool, just_enabled: bool, due: bool) -> PanePass` (old `pane_pass`, renamed) and its publish step `app::publish_graph_state(wanted: bool, full_pass_done: bool, prev_ready: bool) -> GraphPublish { ready: bool, clear: bool }`
- **When**: `publish_graph_state` is called for every input combination
- **Then**: `wanted && full_pass_done` → `ready: true`; `!wanted` → `ready: false, clear: true` (every `repo`, `PaneMeta` and pane list published empty); `wanted && !full_pass_done` → `ready: prev_ready, clear: false`; and `graph_pass` keeps old R6.6's eight-row table

### R5: The graph's shape — D1, D2, D3

#### R5.1: One project, main sessions before worktree sessions
- **Given**: rows for tabs 1–4 in strip order: tab 1 in worktree `w` of repo R, tab 2 in R's main worktree, tab 3 with no Claude agent, tab 4 in R's main worktree
- **When**: `app::build_graph(rows, metas, home, subagents, folds)` is called
- **Then**: the nodes are, in order: `Project(R)` (column 0), session tab 2, session tab 4, session tab 1 (column 1, each `parent` = the project); tab 3 yields no node

#### R5.2: The project card's content
- **Given**: repo R with `main_root` `~/IdeaProjects/terminal` and only a linked-worktree session
- **When**: `build_graph` is called
- **Then**: the project node exists with title `terminal`, folder `~/IdeaProjects/terminal`, no branch, `CardStatus::Hidden`, `AgentSkin::Generic` and `FlowTarget::None`

#### R5.3: Groups ordered by first session; non-git sessions last, unparented
- **Given**: tab 1 in repo B, tab 2 outside any repository, tab 3 in repo A
- **When**: `build_graph` is called
- **Then**: the order is `Project(B)`, tab 1, `Project(A)`, tab 3, then tab 2 with `parent: None` in column 1

#### R5.4: A split tab yields one card per pane session
- **Given**: a split tab whose panes 10 (active, Claude) and 11 (Claude) run in R's main worktree and in R's linked worktree `w`, and pane 12 runs no agent
- **When**: `build_graph` is called
- **Then**: two session nodes exist under `Project(R)` — pane 10's in the main group, pane 11's in the linked group — and none for pane 12; pane 11's folder is its own cwd

#### R5.5: Session card content is the card's `RowMeta`
- **Given**: an unsplit Claude tab with a row and meta
- **When**: `build_graph` is called
- **Then**: its node's title, folder, branch, skin and status equal `row_meta(row, meta, home)`'s `line1`, `cwd_display`, `branch`, `skin` and `status`

#### R5.6: The active tab's session is selected
- **Given**: three Claude tabs with tab 2 active, tab 2 split with pane 21 active
- **When**: `build_graph` is called
- **Then**: exactly one node has `selected: true` — pane 21's session

### R6: Session status equals the card's — old D2, old D13

#### R6.1: Unsplit session and active pane take the card's status
- **Given**: for each `AgentState` value and both `blocking_dialog` values, an unsplit row with a Claude meta; and a split tab with `blocking_dialog: true`, `TabMeta.agent` Claude/Busy, active pane 10 and pane 11 Claude/Busy
- **When**: `build_graph` is called
- **Then**: the unsplit session's status equals `row_meta(..).status` in every combination; pane 10 reads `WaitingForInput` and pane 11 reads `Working`

#### R6.2: Non-active pane is log-only; unknown is undotted
- **Given**: a non-active pane with Claude/Waiting and `blocking_dialog: false`; an unsplit Claude meta with `AgentState::Unknown`
- **When**: `build_graph` is called
- **Then**: the pane's status equals `resolve_status(Some(&agent), false, false, None)`; the unknown one is a session node with `CardStatus::Hidden`

### R7: The header — old D3, D16

#### R7.1: Count format and what is counted
- **Given**: five sessions (two Working, one WaitingForInput, one Ready, one Hidden), two project nodes and three running subagents
- **When**: `FlowGraph::header()` is read
- **Then**: it is exactly `2 working · 1 waiting / 5`; with no sessions it is `0 working · 0 waiting / 0`

#### R7.2: Resolving until the first full pass
- **Given**: an open `FlowOverlay` whose tabmeta snapshot has `graph_ready: false`, with two Claude tabs published
- **When**: the overlay builds its model (`FlowOverlay::poll`)
- **Then**: the model has no nodes and its header reads `resolving…`; once a snapshot with `graph_ready: true` is synced, the next build has the nodes and the count header

### R8: Subagents and folds — D8, old D15

#### R8.1: Running subagents as cards
- **Given**: a session whose log maps to subagents `{type: "Explore", description: "find x"}`, `{type: None, description: "fix"}`, `{type: None, description: None}`, all running
- **When**: `build_graph` is called
- **Then**: three `Subagent` nodes in column 2 with `parent` = that session, in log order, titled `Explore`, `fix`, `agent`, with folders `find x`, empty, empty, status `Working` and the session's skin

#### R8.2: Finished subagents fold behind one node
- **Given**: the same session plus four finished subagents and an empty fold set
- **When**: `build_graph` is called
- **Then**: after the running nodes there is exactly one `Fold` node labelled `▸ 4 finished` with `FlowTarget::Fold(tab, pane_pid)` and no finished subagent nodes; a session with none finished has no `Fold` node

#### R8.3: Expanded, finished subagents are muted cards
- **Given**: the fold set contains that session's `(TabId, pane_pid)`
- **When**: `build_graph` is called
- **Then**: the `Fold` node reads `▾ 4 finished` and is followed by four `Subagent` nodes with `muted: true` and `CardStatus::Hidden`

#### R8.4: Folds are per session and pruned
- **Given**: two sessions sharing one log with only the first's key in the fold set
- **When**: `build_graph` is called, then called again after the first session's pane disappears
- **Then**: only the first is expanded; the second call's returned fold set no longer holds the vanished key

### R9: Clicks — D7

#### R9.1: Renderer hit-test by node
- **Given**: a `FlowHandle` with a laid-out graph of a project, a session (tab 7), a subagent (tab 7) and a fold (tab 7, pane 10)
- **When**: `FlowHandle::press([x, y])` is called at the centre of each card or chip, on an edge midpoint, and on empty space
- **Then**: it returns `None`, `Some(FlowHit::Node(7))`, `Some(FlowHit::Node(7))`, `Some(FlowHit::Fold(7, 10))`, `None`, `None`

#### R9.2: A card click activates its tab and closes the graph
- **Given**: an open `FlowOverlay` whose handle resolves the point to `FlowHit::Node(7)`
- **When**: `Overlay::on_press(point, ctx)` is called
- **Then**: it returns `Consumed`, `ctx` holds exactly `AppRequest::ActivateTab(TabId(7))`, and `visible()` is false

#### R9.3: A fold click toggles and stays open
- **Given**: an open `FlowOverlay` whose handle resolves the point to `FlowHit::Fold(7, 10)`
- **When**: `on_press` is called twice
- **Then**: each returns `Consumed` with no request; `folds()` contains `(TabId(7), 10)` after the first and not after the second; `visible()` stays true

#### R9.4: ActivateTab resolves through the strip
- **Given**: tab ids `[TabId(1), TabId(7), TabId(9)]`
- **When**: `app::tab_position(ids, id)` is called with `TabId(7)` and `TabId(4)`
- **Then**: `Some(1)` and `None` (old R8.3, unchanged)

### R10: Layout — D9, D12

#### R10.1: Slots and columns
- **Given**: a graph of one project with two sessions, the first with three running subagents and the second with none, in a 1600×1000 viewport at scale 1
- **When**: `renderer::graph_layout::layout_graph(graph, viewport, scale, metrics, sidebar_w)` (`sidebar_w` = `[sidebar].width_px`, logical px) is called
- **Then**: the project and the first session start in slot 0, the subagents occupy slots 0–2, the second session slot 3; every card is `card_h` tall; column `k`'s cards share one `x`, increasing with `k`; no two cards overlap

#### R10.2: Card width shrinks to fit three columns, with a floor
- **Given**: viewports 1600, 700 and 500 px wide at scale 1, with `[sidebar].width_px` 280
- **When**: `layout_graph` is called
- **Then**: at 1600 the card width is the sidebar card's width; at 700 it is the largest width that fits three columns, gaps and margins, and ≥ `MIN_CARD_W`; at 500 column 2 is not laid out

#### R10.3: Narrow modes
- **Given**: the R10.1 graph in a viewport where three columns do not fit at `MIN_CARD_W`, then one where two do not fit, then one narrower than one card
- **When**: `layout_graph` is called on each
- **Then**: (a) each session takes one slot and the first shows a `+3` chip, inert in hit-testing; (b) only session cards are laid out, one column, in group order; (c) only the header is laid out

#### R10.4: Edges are sampled S-curves between card midpoints
- **Given**: the R10.1 layout
- **When**: its edges are read
- **Then**: there is one edge per parent→child pair (5); each has `EDGE_SEGMENTS` (16) capsules whose chain starts at the parent card's right-edge midpoint and ends at the child's left-edge midpoint (±0.5 px), with consecutive capsules sharing endpoints, and one end ring at the child

### R11: Scroll by slot — D13

#### R11.1: Offset clamps by slot
- **Given**: 60 slots of which 20 fit below the header
- **When**: `renderer::graph_panel::scroll_offset(current, notches, total, visible)` is called from 0 with +100, then from that result with −100, and with `total < visible`
- **Then**: 40, 0, and 0

#### R11.2: Culling draws whole nodes, and edges only with both ends
- **Given**: the R10.1 graph with one running subagent added to the second session (so the second session and its subagent share slot 3), laid out and scrolled by 1 slot in a viewport that fits 3 slots
- **When**: the composed draw list is read
- **Then**: the project card and the first session's card (slot 0) are not drawn, nor any edge from them; the subagents in slots 1–2, the second session and its subagent in slot 3 are drawn; the second session → its subagent edge **is** drawn; and every drawn edge has both its end cards drawn

#### R11.3: The wheel scrolls the graph
- **Given**: an open `FlowOverlay` over a graph taller than the viewport
- **When**: `Overlay::on_wheel` receives 3 notches down
- **Then**: the handle's scroll offset is 3 slots (clamped per R11.1) and it returns `Consumed`

### R12: Rendering and cost — D4, D11, D14, D17

#### R12.1: Cards, edges and colours reach the screen
- **Given**: a headless renderer (`renderer::testing::headless`) with the ai-monitor open and a visible `FlowPanel` holding a project, a Working, a WaitingForInput and a Ready session, and one running subagent
- **When**: `Renderer::capture` is called
- **Then**: every card rect carries glyph ink; each session card's status area contains its sidebar status colour; pixels at an edge's sampled midpoint differ from the backdrop; and no pixel of the terminal grid or the ai-monitor band differs from the backdrop outside the graph's cards, edges and header

#### R12.2: Unchanged key, no rebuild
- **Given**: a `FlowPanel` that has recorded one frame
- **When**: ten more frames record with the same graph, scroll and hover
- **Then**: `layout_count()` stays 1 and `compose_count()` stays 1

#### R12.3: A change rebuilds on the next frame, scroll and hover only re-compose
- **Given**: the same panel
- **When**: one session's status changes, then the scroll offset changes, then the hovered node changes
- **Then**: `layout_count()` becomes 2 on the status change and stays 2; `compose_count()` increases on each of the three

#### R12.4: Nothing is built while closed
- **Given**: a `FlowPanel` whose handle has never been visible
- **When**: ten frames record
- **Then**: `FlowPanel::atlas_builds()` (its own `CardAtlases::builds()`) is 0, `layout_count()` is 0 and `submit_count()` (frames in which it submitted any draw) is 0

### R13: Carried workers — old D9, old D10, old D14, old D18

#### R13.1: tmux panes and telemetry stand
- **Given**: the crate after this change
- **When**: the old R6.1–R6.4, R6.6, R6.7 (tmux parsing, `panes_of`, the pass gate) and R7.1–R7.4 (telemetry thread) acceptance tests run, with `set_panes_wanted` renamed `set_graph_wanted` and `pane_pass` `graph_pass` (old R6.5's field lock is replaced by R4.6)
- **Then**: they pass unchanged in assertion

### R14: Every detected agent is a session — D1, D3, D8, D10 (amended 2026-10-21)

#### R14.1: Non-Claude agents are session cards, grouped by repository
- **Given**: unsplit tabs 1–4 in strip order, all in repo R's main worktree: tab 1 runs Antigravity, tab 2 Codex, tab 3 Gemini, tab 4 Aider; tab 5 runs no agent
- **When**: `build_graph` runs
- **Then**: one project card for R, then four session cards in tab order 1–4 under it; no node for tab 5; each session card's title, folder, branch, skin and status equal that tab's `row_meta`

#### R14.2: Mixed agents in one split tab
- **Given**: a split tab whose active pane runs Claude in R's main worktree and whose non-active pane runs Codex in R's linked worktree `w`
- **When**: `build_graph` runs
- **Then**: two session cards under R's project — the Claude pane's first, then the Codex pane's — and the Codex card's status is `resolve_status(pane.agent, false, false, None)`

#### R14.3: Subagents only under Claude
- **Given**: a Codex tab and a Claude tab whose published logs are both keys in the subagent map, each with one running and one finished subagent
- **When**: `build_graph` runs with an empty fold set
- **Then**: the Claude session has one Subagent node and one Fold node after it; the Codex session has no child nodes

#### R14.4: The header counts every agent session
- **Given**: a graph built from an Antigravity tab (Working), a Codex tab (Waiting for input) and a Claude tab (Ready)
- **When**: `FlowGraph::header()` is read
- **Then**: it reads `1 working · 1 waiting / 3`

#### R14.5: The worker resolves a repository for every agent kind
- **Given**: the graph is wanted and a tab whose detected agent is not Claude (e.g. Codex) runs in a linked worktree of R
- **When**: the tabmeta worker derives that tab's meta
- **Then**: `TabMeta.repo` is `Some(RepoId { common: R/.git, main_root: R, linked: true })`; a tab with no agent publishes `repo: None`

## Tasks

In the requirements, `FlowGraph`, `FlowHandle`, `FlowPanel` and `scroll_offset` name
the items in the new `renderer::graph_layout` / `renderer::graph_panel` modules (T3),
not the session-flow-viewer ones in `flow_layout` / `flow_panel`, which T4 deletes.

Work starts from the uncommitted `feat/session-flow` tree (session-flow-viewer as
built). Acceptance tests are written first, in **new** files prefixed `tt{n}_` under
`crates/*/tests/`. The old session-flow-viewer TT files and their fate:
- `crates/app/tests/{tt1_dock,tt2_flow_tree,tt3_flow_status,tt4_flow_header,tt5_flow_subagents,tt8_flow_click}.rs`
  → **deleted by T1 as its first step**: they import the dock seam T1 removes or build
  the `TabMeta` literals T2 extends, and they test behaviour this spec supersedes, so
  deleting them first keeps every test target compiling from T1 on.
- `crates/renderer/tests/{tt8_flow_press,tt9_flow_panel}.rs` → deleted by T4 together
  with the `flow_layout` / `flow_panel` modules they test (which compile untouched until then).
- `crates/app/tests/tt6_tmux_panes.rs` and `tt7_flow_telemetry.rs` stay (R13) and are
  edited only by T2: the renames `set_panes_wanted` → `set_graph_wanted` and
  `pane_pass` → `graph_pass`, and the deletion of `r6_5_tab_meta_has_exactly_seven_fields`
  (`tt6_tmux_panes.rs:97-124`), which R4.6's in-crate field lock replaces.

**When a task is done.** TTs are written test-first, so a TT file is red — it may not
even compile — from when it lands until the implementation task that builds its
symbols finishes. Each implementation task is therefore judged on the crate it builds,
with every TT that targets it green: T1 and T2 by `cargo test -p app` and `cargo clippy
-p app --all-targets -- -D warnings`, T3 by the same for `-p renderer`. TT ordering
keeps those boundaries clean: no TT lands in a crate while an earlier task on that
crate is still running (TT4 waits for T1; TT10 and TT12 touch only the renderer; the
TTs that target T4 wait for T1–T3). T4, and the verification gate after it, are judged
on the whole workspace: `cargo test --workspace` and `cargo clippy --workspace
--all-targets -- -D warnings`. New TT file names use the prefixes `tt1_`…`tt12_` with a
`graph_` word (`tt1_graph_keys.rs`, …) so none collides with a surviving old file.
`specs/session-flow-graph/tt-hashes.txt` is written once every TT task has finished.

### T1: Revert the shared dock; route the pointer to an open graph [infra]
- **Fulfills**: R2.1, R2.2, R3.1
- **Depends on**: (none) — parallel with T3
- **Files**: `crates/app/src/{seam,overlay,terminal_app,rename,input,lib,flow}.rs`, `crates/app/tests/wiring.rs`, the six old app TT files listed above (deletion)
- **Work**: Remove `RightDock`, `Occupant`, `AppRequest::ToggleDock`,
  `OverlayCommand::DockChanged` and `Running::toggle_dock`; restore
  `AppRequest::SetMonitorOpen(bool)` and `OverlayCommand::RestoreMonitor` with their
  `main` shape (`git show main:crates/app/src/seam.rs`, `overlay.rs`, `terminal_app.rs`)
  — the monitor toggles and persists exactly as on `main`. Keep `AppRequest::ActivateTab`,
  `on_press(point, &mut AppCtx)`, `tab_position` / `Tabs::position`. Add the pure
  `input::pointer_owner(graph_visible, in_monitor_band, in_sidebar) -> PointerOwner`
  and make `terminal_app.rs`'s press, wheel and cursor-moved paths call it, sending
  `Graph` to `OverlayHost` (press/wheel/hover of the visible graph overlay) before any
  other handler. Overlays gain `fn on_hover(&mut self, point: Option<[f32; 2]>) {}` (default
  no-op) for D3's hover. Re-export `pointer_owner`, `PointerOwner` from `lib.rs`; drop
  the `RightDock` / `Occupant` re-exports. `FlowOverlay` keeps compiling against the
  reverted seam with its dock code stubbed out (T4 replaces it).

### T2: Repository identity and graph readiness in the tabmeta worker [infra]
- **Fulfills**: R4, R13
- **Depends on**: T1 (both edit `crates/app/src/lib.rs`'s re-export list) — parallel with T3
- **Files**: `crates/app/src/tabmeta/{git,worker,mod,proc}.rs`, `crates/app/src/lib.rs`, `crates/app/src/flow.rs` (the `set_panes_wanted` call site and its `TabMeta`/`PaneMeta` test literals, `flow.rs:663-674`), the exhaustive `TabMeta` test literals in `crates/app/src/{session,sidebar_model,terminal_app}.rs` (`session.rs:478`, `sidebar_model.rs:333`, `terminal_app.rs:3555`), `crates/app/tests/tt6_tmux_panes.rs`, `crates/app/tests/tt7_flow_telemetry.rs` (rename only)
- **Work**: `git::repo_of(cwd) -> Option<RepoId>` per D10 (main, linked via
  `commondir`, submodule, none; path normalisation), memoised by cwd with per-tick
  `stat` revalidation of the found `.git` (+ `commondir`) and the existing
  `NEG_MEMO_TTL_TICKS` for misses, counted by the `probe` counters. The worker resolves
  each session's cwd from `proc::cwd_of_pid(agent_pid)` (fallback: shell cwd) and
  publishes `TabMeta.repo`, `PaneMeta.{cwd, branch, repo}` and `TabMetaShared.graph_ready` per
  D10, via the pure `publish_graph_state`; `set_panes_wanted` → `set_graph_wanted`,
  `pane_pass` → `graph_pass`. R4.5 (memo `stat` cost, via the `pub(crate)` `probe`
  counters) and R4.6 (field locks, including `TabMetaShared.graph_ready`) are
  **in-crate unit tests** in `tabmeta/{git,worker}.rs`, since neither the memo nor
  `TabMetaShared` is public. `TabMetaSnapshot` (`tabmeta/mod.rs:368`) gains
  `pub fn graph_ready(&self) -> bool`, copied from `TabMetaShared` on sync, and a public
  test constructor `TabMetaSnapshot::for_test(entries: Vec<(TabId, TabMeta)>,
  graph_ready: bool)`; re-export `TabMetaSnapshot`. Re-export `repo_of`,
  `RepoId`, `graph_pass`, `publish_graph_state`, `GraphPublish` (each with its
  `/// crates/app/tests/tt4_….rs` consumer line).

### T3: Renderer graph — layout, edges, scroll, full-window layer [infra]
- **Fulfills**: R3.3, R9.1, R10, R11.1, R11.2, R12
- **Depends on**: (none) — parallel with T1 and T2
- **Files**: `crates/renderer/src/{graph_layout,graph_panel,sidebar_row,sidebar_text,lib}.rs` (`graph_*` new). `flow_layout.rs` / `flow_panel.rs` are **not** touched, so the app crate keeps compiling against them until T4 — this is what lets T3 run in parallel with T1 and T2.
- **Work**: Update `Renderer::set_monitor_open`'s doc comment back to the monitor-only band. In new `pub mod graph_layout` define `FlowGraph { nodes: Vec<FlowNode>, resolving: bool }`
  (`FlowNode { kind: NodeKind::{Project, Session, Subagent, Fold}, parent: Option<usize>,
  title, folder, branch: Option<String>, skin: AgentSkin, status: CardStatus, muted,
  selected, target: FlowTarget, extra: u32 /* +N in narrow mode */ }`, plain data) and
  `FlowGraph::header()`. Pure `layout_graph(graph, viewport, scale, metrics: CardMetrics, sidebar_w: f32)` → slots,
  card rects, fold chips, `+N` chips, edges (`EDGE_SEGMENTS` = 16 capsules + end ring),
  per D9/D12, with `COL_GAP` = 48, `MARGIN` = 24, `ROW_GAP` = `CardMetrics.gap`,
  `HEADER_H` = 40 and `MIN_CARD_W` = 180 (logical px, × scale); the column-2 slot of a
  session's first subagent is its own slot. `compose(layout, scroll, hover)` culls per D13.
  In new `pub mod graph_panel`: `FlowHandle { set,
  set_visible, press, hover, scroll, set_scroll }` and `scroll_offset`; `press(point) ->
  Option<FlowHit>` with `FlowHit { Node(u64), Fold(u64, u32) }` defined in `graph_panel`
  (cards and the fold chip only; edges, the project card, `+N` chips and empty space →
  `None`). `FlowPanel` at
  z 110 draws a full-viewport backdrop, edges, cards via `build_card` with its own
  `CardAtlases` (built on first visible frame), chips and header; `layout_count()`,
  `compose_count()`, `atlas_builds()`, `submit_count()`. Factor `TabBar::record`'s card-run resolution
  (`sidebar_row.rs:1531-1600`'s text-run → atlas instance step) into one shared
  function both layers call, behaviour-preserving for the sidebar (its existing tests
  and `golden_frame.rs` stay green).

### T4: The graph overlay — builder, keys, clicks, wheel, wiring [vertical]
- **Fulfills**: R0, R1, R2.3, R3.2, R5, R6, R7, R8, R9.2, R9.3, R9.4, R11.3
- **Depends on**: T1, T2, T3, TT1, TT2, TT3, TT5, TT6, TT7, TT8, TT9, TT11
- **Files**: `crates/app/src/flow.rs`, `crates/app/src/lib.rs`, `crates/app/tests/wiring.rs`, `crates/renderer/src/{flow_layout,flow_panel,lib}.rs` (deletion), `docs/architecture.md`, `docs/domain.md`
- **Work**: `build_graph(rows, metas, home, subagents, folds) -> (FlowGraph, FoldSet)`
  (as built it takes a trailing `active: usize`, the strip index of the active tab from
  `SidebarModel.active`, for D3's `selected` — no `SidebarRow` carries it) per D1–D3, D8 and old D2/D13 (grouping by `RepoId.common`, order, project card,
  non-git tail, pane cards, selected, subagent cards, fold node, pruning); `resolving`
  until `graph_ready`. `FlowOverlay`: priority 300; key policy per D5; `set_graph_wanted`
  on open/close; telemetry thread lifecycle unchanged (old D10); `on_press` per D7
  (activate + close / fold toggle / consume); `on_wheel` → slot scroll; `on_hover`.
  Re-export `build_graph`. Delete `crates/renderer/src/flow_layout.rs` and
  `flow_panel.rs` (and their `pub mod` lines) once `flow.rs` uses `graph_*`, with their
  tests `crates/renderer/tests/{tt8_flow_press,tt9_flow_panel}.rs`. For R7.2,
  `FlowOverlay::sync_snapshot(&mut self, snap: TabMetaSnapshot)` is the test seam (the
  overlay's `poll` calls it with the handle's snapshot). `wiring.rs`: 6 overlays, the graph's priority highest. Docs:
  rewrite architecture G10 (graph overlay, the dock is the monitor's again, the three
  limits that still apply) and domain.md's *right dock* / *flow panel* rows.

### TT1: acceptance tests for R1's GWTs [test]
- **Fulfills**: R1 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/app/tests/tt1_graph_keys.rs`

### TT2: acceptance tests for R2's GWTs [test]
- **Fulfills**: R2 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/app/tests/tt2_graph_pointer.rs`

### TT3: acceptance tests for R3's GWTs [test]
- **Fulfills**: R3 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/app/tests/tt3_graph_monitor.rs` (R3.1–R3.2), `crates/renderer/tests/tt3_graph_inset.rs` (R3.3)

### TT4: acceptance tests for R4's GWTs [test]
- **Fulfills**: R4 (tests)
- **Depends on**: T1
- **Creates**: `crates/app/tests/tt4_graph_repo.rs` (R4.1–R4.4, R4.7; R4.5–R4.6 are T2's in-crate unit tests, see T2)

### TT5: acceptance tests for R5's GWTs [test]
- **Fulfills**: R5 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/app/tests/tt5_graph_shape.rs`

### TT6: acceptance tests for R6's GWTs [test]
- **Fulfills**: R6 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/app/tests/tt6_graph_status.rs`

### TT7: acceptance tests for R7's GWTs [test]
- **Fulfills**: R7 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/app/tests/tt7_graph_header.rs`

### TT8: acceptance tests for R8's GWTs [test]
- **Fulfills**: R8 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/app/tests/tt8_graph_subagents.rs`

### TT9: acceptance tests for R9's GWTs [test]
- **Fulfills**: R9 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/renderer/tests/tt9_graph_press.rs` (R9.1), `crates/app/tests/tt9_graph_click.rs` (R9.2–R9.4)

### TT10: acceptance tests for R10's GWTs [test]
- **Fulfills**: R10 (tests)
- **Depends on**: (none)
- **Creates**: `crates/renderer/tests/tt10_graph_layout.rs`

### TT11: acceptance tests for R11's GWTs [test]
- **Fulfills**: R11 (tests)
- **Depends on**: T1, T2, T3
- **Creates**: `crates/renderer/tests/tt11_graph_scroll.rs` (R11.1–R11.2), `crates/app/tests/tt11_graph_wheel.rs` (R11.3)

### T5: Every detected agent is a session (amendment 2026-10-21) [vertical]
- **Fulfills**: R14
- **Depends on**: T4, TT14
- **Files**: `crates/app/src/flow.rs`, `crates/app/src/tabmeta/worker.rs` (and `mod.rs` if needed), `docs/architecture.md` G10, `docs/domain.md`
- **Work**: replace the Claude-only session test in `build_graph` with "any detected agent"; subagents/fold only for Claude sessions; the worker resolves `repo` for any detected agent's pid; docs say "agent session".

### TT14: acceptance tests for R14's GWTs [test]
- **Fulfills**: R14 (tests)
- **Depends on**: T4
- **Creates**: `crates/app/tests/tt14_graph_agents.rs` (R14.1–R14.4); R14.5 as an in-crate worker test written by T5 (the worker's derive path is not public), or in this file if a public seam exists

### TT12: acceptance tests for R12's GWTs [test]
- **Fulfills**: R12 (tests)
- **Depends on**: (none)
- **Creates**: `crates/renderer/tests/tt12_graph_frame.rs`

Infra tasks never wait on a TT. TT4 is written in parallel with T2, and TT10 and TT12
in parallel with T3, test-first. TT1–TT3, TT5–TT9 and TT11 target T4's symbols
(`build_graph`, `sync_snapshot`, the key policy, priority 300), so they wait for T1–T3,
are red until T4 lands, and are part of T4's done check; TT2 and TT3 also hold T1's
R2.1–R2.2 and R3.1, which T1 itself checks through its in-crate tests and which
those TTs re-check at T4.

## Spec Review
- **Verdict**: NEEDS_FIX → both Open items resolved by the user at Final Approval (2026-10-20)
- **Reviews**: 3
- **Fixed**:
  - [Task order] T1 ∥ T3 — T1's `crates/renderer/src/lib.rs` doc edit moved into T3.
  - [Task order/Readiness] T3 — the new renderer code goes in new `graph_layout` / `graph_panel` modules, so the app keeps compiling against the old ones until T4 deletes them.
  - [Testability] R4.5, R4.6 — made T2's in-crate unit tests (the memo, `probe` and `TabMetaShared` are `pub(crate)`).
  - [Readiness] R7.2 — `TabMetaSnapshot::graph_ready` + `for_test` (T2) and `FlowOverlay::sync_snapshot` (T4) named as the seam.
  - [Readiness] D12/R10.2 — `COL_GAP`, `MARGIN`, `ROW_GAP`, `HEADER_H`, `MIN_CARD_W` given values in T3; `layout_graph` takes `sidebar_w`.
  - [Testability] R12.4 — `FlowPanel::atlas_builds()` / `submit_count()` named.
  - [Grounding] R3.1 — `OverlayHost::key` → `OverlayHost::dispatch`.
  - [Readiness] T2/R13.1 — old R6.5 field-lock test deleted by T2, replaced by R4.6.
  - [Task order] old TT files — the six old app TT files are deleted by T1 first; the two renderer ones by T4 with their modules.
  - [Coverage] R2.3, R3.2 — moved from T1 to T4.
  - [Consistency] D15, D17 — wording made to match the task list.
  - [Testability] R11.2 — "if it fits" hedge removed.
  - [Task order] TT4/TT10/TT12 and the T4-targeting TTs — done criteria stated per crate and per task; TT4 depends on T1.
  - [Grounding] T2 — the struct-literal sites it breaks are listed in its Files.
  - [Readiness] T3/R9.1 — `FlowHit` defined in `graph_panel`.
- **Open** (resolved at Final Approval — R4.6 gained `PaneMeta.branch`, R11.2 now asserts a visible edge):
  - [Consistency] D3 / R4.6 — a non-active pane card shows "the folder and branch of its own cwd", but R4.6 locks `PaneMeta` to `pane_pid, active, agent, log, cwd, repo` with no `branch`. Needs an approved-requirement change: add `branch` to `PaneMeta` in R4.6 (and T2), or make D3 say a pane card shows the tab's branch.
  - [Testability] R11.2 — in its Given every edge has an end in the culled slot 0, so "every drawn edge has both ends drawn" is vacuously true. Needs an approved-requirement change: give the second session a subagent (slot 3) and assert that edge is drawn.

## Post-verification amendments (2026-10-21)
All four were found by the verification review and approved by the user on 2026-10-21.
- **D9** — edges drawn at `EDGE_ALPHA` = 0.45 (muted: × `MUTED_ALPHA`) instead of
  `CARD_BORDER_ALPHA` (0.10), which was barely visible. Pinned by
  `graph_layout::tests::d9_an_edge_stands_clearly_above_the_backdrop`.
- **D5** — a card menu open when the graph opens survives the graph: neither the
  consumed-key dismissal nor the Esc dismissal applies while the graph covers the window
  before or after the key. Pinned by
  `terminal_app::tests::d5_a_card_menu_is_still_open_after_the_graph_opens_and_closes_over_it`.
- **D10** — the reopen race: readiness carries the open's epoch, and the overlay ignores
  a readiness from an earlier open. Pinned by
  `flow::tests::d10_a_reopen_reads_resolving_until_the_worker_answers_this_open`.
- **D3** — the spec now matches the code: a non-active pane card takes its own agent's
  skin, not the tab's.

### Every detected agent is a session (user decision 2026-10-21)
After seeing the graph, the user asked for sessions of **every agent the sidebar detects**
— Claude, Antigravity, Codex, Gemini and Aider — not only Claude. This amends:
- **D1 / old D5 (scope)** — a *session* is a tab (unsplit) or a pane (split) whose agent
  is any detected `AgentKind`. A tab or pane with no agent is still not a node. Wherever
  D1, D3, D10 or R5–R7 say "Claude session" they now read "agent session"; the header's
  `/ n` counts every agent session.
- **D3** — a session card is still the sidebar card through `row_meta` (title, folder,
  branch, skin, status), so an Antigravity card looks and reads exactly as its sidebar
  card does. A non-active pane's status stays `resolve_status(pane.agent, false, false,
  None)`, whatever the kind.
- **D10** — repository identity is resolved from **the detected agent process's** cwd
  (any kind), with the same shell-cwd fallback. Tabs and panes with no agent publish
  `repo: None`, as before.
- **D8 (unchanged, now stated)** — subagents and the fold node exist only under **Claude**
  sessions: they come from Claude's session log (`flow_telemetry`), which the other agents
  do not write. A non-Claude session has no column-2 children.
- Implemented as **T5**, test-first as **TT14** (R14).

### Graph ignores `[sidebar].show_agent_status` (user decision 2026-10-22)
Review found the toggle split the graph: with it off, an unsplit tab or active pane published
`agent: None` beside a resolved `repo` and produced no session, while non-active panes
(`Deriver::pane`, never behind the toggle) still did. The user decided that toggle is the
sidebar card's display preference only; the graph always detects agents. This amends:
- **D3 / D10** — on a pass that derives the graph (`PanePass::Derive`) the worker walks and
  classifies every tab's agent whatever the toggle, through the same `activity` chain, and
  publishes the result as `TabMeta.graph_agent` / `graph_task` (set only on graph passes,
  cleared by `strip_graph` like `panes` and `repo`). `TabMeta.agent` / `task` keep the toggle,
  so the sidebar card, the title and the dialog scan are unchanged. A graph session's card is
  still `row_meta`'s join (`sidebar_model::row_meta_with`, fed `graph_agent`/`graph_task`, and
  `agent`/`task` when no graph copy is published), so status has one classifier and is the
  real status: with the toggle off the graph is identical to the toggle-on graph.
- **G1** — with the graph closed nothing changes: the walk gate stays `enabled_agent ||
  is_active` and no tab is classified while the toggle is off.
- Pinned by `tabmeta::worker::tests::toggle_off_*` (four tests).

### Empty-state hint (user decision 2026-10-23)
In manual testing a fresh window (one plain zsh tab) opened the graph to a bare
`0 working · 0 waiting / 0` with nothing beneath it, which reads as broken: the user's agents
were running in other windows, which the graph never counts. The user asked for a hint. This
amends R7 with:

#### R7.3: An empty graph says why
- **Given**: a `FlowGraph` that is not `resolving` and has no `Session` node (no nodes, or
  project nodes only)
- **When**: `FlowGraph::empty_hint()` is read, and the layout is composed
- **Then**: it is `Some(EMPTY_HINT)` (`No agent sessions in this window. Start claude, codex,
  agy, gemini or aider in a tab.`), drawn as one `Meta`-face line in the card meta colour at
  `list_top`, truncated with the ellipsis to the window width and omitted when a whole line
  does not fit; while `resolving`, or with any session, it is `None` and nothing is drawn.
  The header string is unchanged (R7.1 still holds).
- Pinned by `graph_layout::tests::the_empty_hint_shows_only_when_resolved_with_no_session`
  , `an_empty_graph_draws_the_hint_under_the_header` and
  `the_empty_hint_is_placed_and_cut_exactly`.

## External Dependencies

### Pre-work
- (none)

### Post-work
- Manual check R0.1 in the running app (`/run-term`), with one `claude` in a main worktree and one in a linked worktree.
- `specs/session-flow-viewer/spec.md` Meta gains `- **Superseded in part by**: specs/session-flow-graph/spec.md` (D1, D4, D6, D7, D8 presentation, D11 key, D12, D16, D17).
- Committing is the user's call; nothing in this plan commits.
