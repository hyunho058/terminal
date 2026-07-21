# partition-manifest.md — sidebar-redesign
- generated_at: 2026-07-19
- goal: Redesign the left tab sidebar of mac-terminal to match the Claude Design mock `Mac Terminal.dc.html` (project 2ae889c6-09a6-402b-8675-9d5bafd3aa88) — rounded two-line tab cards on a wider configurable sidebar, with two new per-tab data features: the tab's git branch name, and whether an AI agent CLI is running in that tab (which agent, and its state).

## Shared Decisions

### SD1: Adapt the mock's visual language onto the hand-rolled wgpu renderer — never clone its DOM
- **Status**: resolved
- **Rationale**: `Mac Terminal.dc.html` is a CSS/DOM/SVG mock. We reproduce its *visual language* — the `#1e1b18` sidebar on `#24211d`, 13px-radius cards, 12/15px card padding, 3/8px card margins, two-line rows, the `✳`/`◇` mark glyphs, the branch sub-line, the per-agent gradient skins, the 340px default width — as instanced quads and WGSL on the existing renderer. Rejected: adding any GUI/widget crate to get flexbox/SVG for free (violates `.claude/rules/no-gui-crates.md`, the project's reason to exist). Rejected: pixel-perfect cloning — font rasterization, antialiasing and subpixel metrics differ between a browser and our swash atlas, so "matches the mock" means the design tokens are honored, not that a screenshot diff is empty.

### SD2: Sidebar chrome lives in physical-px space, with ONE geometry source of truth shared by draw and hit-test
- **Status**: resolved
- **Rationale**: The mock's paddings, radii and gaps are not cell multiples, so cell-space layout cannot express them. All sidebar layout stays in physical px as pure functions in the renderer, and the app hit-tests by calling those same functions — extending the existing `slot_y`/`tab_at` exact-inverse pair (`crates/renderer/src/tab_bar.rs:98,107`) that `input::tab_target` already delegates to. The sidebar's px width reaches the terminal grid only through `Renderer::set_left_inset` (`crates/renderer/src/lib.rs:508`), which already shifts the grid in the vertex shaders and drives `grid_dims`. Rejected: expressing sidebar layout in cells; rejected: duplicating layout math on the app side (guarantees drift between what is drawn and what is clickable).

### SD3: `term-core` stays GPU-free AND tab-free; tab/git/agent state crosses to the renderer as plain data
- **Status**: resolved
- **Rationale**: `.claude/rules/dependency-direction.md` makes the headless core load-bearing. Tabs are already an `app`-only concept (`crates/app/src/tabs.rs`); git and agent state are likewise app concerns. The renderer keeps receiving them over the existing `Rc<RefCell<TabBarModel>>` side-channel (`tab_bar.rs:30-54`), extended to carry the richer per-tab payload — never on `GridSnapshot`. Rejected: adding tab/git/agent fields to `GridSnapshot` (it is the versioned grid contract and would pull non-terminal state into `term-core`).

### SD4: Per-tab metadata is polled off the UI thread and cached; the render path performs no I/O
- **Status**: resolved
- **Rationale**: `Tabs::titles()` already costs a `proc_pidinfo` syscall per tab per call, and the app runs a continuous redraw loop (`terminal_app.rs:428-430`, the winit#2640 workaround). Adding a filesystem read and a process-tree walk per tab per frame would compound that into a measurable idle-CPU regression — the exact debt G1 records as already paid down once. So a background worker refreshes branch + agent state keyed by `TabId`, and pushes results in via the existing `EventLoopProxy` wake path that PTY output already uses. Rejected: computing branch/agent inline in `sync_tab_model()`.

### SD5: Git branch comes from reading `.git` directly — no git library, no `git` subprocess
- **Status**: resolved
- **Rationale**: We need exactly one string per tab: the current branch. Walking up from the tab's cwd to the first `.git` and reading `HEAD` (`ref: refs/heads/<name>` → branch name; a raw SHA → short detached-HEAD form) is a couple of small file reads. Rejected: `git2` (drags in a libgit2 C build for one field). Rejected: `gix` (large dependency tree for one field). Rejected: shelling out to `git rev-parse` (a process spawn per tab per poll, on a poll loop).

### SD6: AI-agent detection reads the PTY child's descendant processes; `ai-monitor` supplies the Claude-specific detail
- **Status**: resolved
- **Rationale**: `Pty::child_pid()` (`crates/term-core/src/pty.rs:63`) already gives the zsh pid, and the cwd path already uses macOS `libc::proc_pidinfo` (`session.rs:142`) — so a descendant scan via `proc_listchildpids` + `proc_pidpath`, matched against a known-agent table, is the same class of macOS-native call we already depend on (`.claude/rules/macos-only.md` permits this freely). The existing `crates/ai-monitor` JSONL reader supplies the richer Claude Code state where present. Rejected: scraping the terminal's own scrollback for agent banners (fragile, breaks on any CLI release). Rejected: an OSC escape handshake (agent CLIs do not emit one).

### SD7: Sidebar appearance is config-driven, with the mock's values as the defaults
- **Status**: resolved
- **Rationale**: The sidebar's colors, `PAD_X`, `BORDER_W` and the `14.0`/`1.5` cell multipliers are hardcoded in `tab_bar.rs` today, and `common::config` has zero sidebar fields. The mock itself exposes `sidebarWidth` (260–440, default 340), `showGitBranch` and `accent` as knobs, so those are the natural user-facing settings. A new `[sidebar]` section follows its siblings' `#[serde(default, deny_unknown_fields)]` shape and rides the existing live-reload path (`settings::apply`, `settings.rs:23-34`). Rejected: leaving the values hardcoded (the mock's own props argue they are settings).

### SD8: Scope is the SIDEBAR only — the mock's terminal-pane chrome is out, and the search field is deferred
- **Status**: resolved
- **Rationale**: The user's ask is "redesign the left tab", plus git branch and AI agent. The mock's right-hand pane also shows a top-right icon row, a bottom status/footer bar, and an "Use agent" banner — all of which would permanently consume terminal rows from a real PTY grid, and the agent welcome block in the mock is output the agent CLI itself prints (not our chrome). The header's search/filter input is real work of a different kind: a focus-stealing text field must win keystrokes away from PTY routing, which needs its own focus model. Both are deferred to separate future specs rather than smuggled in here. The header's **new-tab** and **hide-sidebar** buttons ARE in scope (they are pure click targets); the filter button is drawn but inert.

## Features  (ordered — a dependency appears before its dependents)

### sidebar-foundations
- declared-surface:
  - crates/common/src/config.rs
  - config.example.toml
  - crates/renderer/src/sidebar_theme.rs
  - crates/renderer/src/sidebar_shapes.rs
  - crates/renderer/src/sidebar_shapes.wgsl
  - crates/renderer/src/sidebar_icons.rs
  - crates/renderer/src/lib.rs
- depends_on: []
- accept-seed: A `[sidebar]` TOML section (width_px, show_git_branch, show_agent_status, collapsed, accent) loads under `deny_unknown_fields` with the mock's values as defaults, and the renderer exposes a rounded-rect primitive with per-instance corner radius, border and gradient fill plus the mock's icon set, covered by pure-geometry unit tests.
- pre-approved-batch: yes

### tab-metadata
- declared-surface:
  - crates/app/src/tabmeta/**
  - crates/app/src/main.rs
  - crates/ai-monitor/src/**
- depends_on: []
- accept-seed: A tab whose cwd is inside a git repo reports its branch name, a tab running an AI agent CLI reports which agent and its state, a plain zsh tab in a non-repo reports neither, and every refresh happens off the UI thread so no render frame performs filesystem or process I/O.
- pre-approved-batch: yes

### sidebar-row-redesign
- declared-surface:
  - crates/renderer/src/tab_bar.rs
  - crates/renderer/src/sidebar_row.rs
  - crates/app/src/sidebar_model.rs
  - crates/app/src/tabs.rs
  - crates/renderer/src/lib.rs
  - crates/app/src/main.rs
- depends_on: [sidebar-foundations, tab-metadata]
- accept-seed: Each tab renders as a rounded two-line card — mark glyph plus title on the first line, branch icon plus branch name on the second — with the active card highlighted and agent-backed cards carrying their skin gradient and border, on a sidebar whose width comes from config.
- pre-approved-batch: yes

### sidebar-chrome-collapse
- declared-surface:
  - crates/renderer/src/sidebar_header.rs
  - crates/renderer/src/sidebar_layout.rs
  - crates/renderer/src/lib.rs
- depends_on: [sidebar-foundations, sidebar-row-redesign]
- accept-seed: The sidebar draws a header row with new-tab and hide buttons above the card list, the list scrolls and clips when tabs overflow the viewport height, and the collapsed state draws only a floating restore button over the terminal — every region exposed as a pure hit-test function that is the exact inverse of what was drawn.
- pre-approved-batch: yes

### sidebar-interaction
- declared-surface:
  - crates/app/src/input.rs
  - crates/app/src/terminal_app.rs
  - crates/app/src/settings.rs
  - crates/app/src/main.rs
- depends_on: [sidebar-row-redesign, sidebar-chrome-collapse]
- accept-seed: Clicking a card switches to that tab, clicking the header buttons opens a new tab or collapses the sidebar, collapsing returns the reclaimed width to the terminal grid and expanding restores it, hovering a card highlights it, and scrolling over the sidebar scrolls the card list.
- pre-approved-batch: yes
