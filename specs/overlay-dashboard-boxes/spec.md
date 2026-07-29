# Spec: overlay-dashboard-boxes

## Meta
- **Created**: 2026-07-28
- **Type**: dev
- **Status**: ready
- **Set**: ai-status-dashboard (`specs/ai-status-dashboard/partition-manifest.md`)
- **Mode**: batch (`pre-approved-batch: yes`) — L0/L2/L3/L4 human prompts skipped under decompose's single partition gate; the L2-reviewer, coverage, GWT and DAG gates all ran
- **Approved by**: decompose partition gate (`specs/ai-status-dashboard/partition-manifest.md`)
- **Approved at**: 2026-07-28

## Declared Surface
- feature-id: overlay-dashboard-boxes
- depends_on: [overlay-dashboard-frame]
- declared-surface:
  - crates/app/src/dashboard.rs

## Goal
Beneath the header, render box-drawn Tasks, Active Agents, Skills, Recent Tools (pass/fail + duration) and File Activity sections, each truncated to fit and each showing an explicit empty-state line when it has nothing to report.

> Two deviations from this wording, both derived from measurement and decided below. **Order**: the panel keeps the *tail* of the composed output, so the frame's summary block is composed **last**, not above the sections — "beneath the header" would guarantee the header is the first thing clipped at the default window (B1). **Naming**: the agents section is titled "Agents", not "Active Agents", because completed entries persist and a heading that claims otherwise is false (B6).

## Non-goals
- The header, the composition helpers, the context-window table or the clock injection — all `overlay-dashboard-frame`, already built
- Colour or styled spans (SD3 — a separate slice)
- Scrolling, focus or any interaction inside the overlay (it is a monitor, not a widget)
- Touching `overlay.rs`, `main.rs`, `seam.rs`, the renderer or `ai-monitor` — this feature owns `dashboard.rs` only

## Confirmed Goal
`crates/app/src/dashboard.rs` gains five section composers appended to the header, in a fixed order, each rendering rows from the corresponding `TelemetrySnapshot` store with per-row truncation, an explicit empty state, and monochrome pass/fail markers. Section order and row caps are this feature's policy (SD4). Everything stays inside the existing pure composer and is `insta`-snapshot tested.

## Research
- `overlay-dashboard-frame` established the composer's shape: a pure `(&TelemetrySnapshot, now_ms) -> Vec<String>`, open-right sections at a nominal `RULE_COLS` (F1), an injected clock (F4), placeholder-not-lie empty handling (F7), and `insta` snapshots (F6). This feature extends that composer and re-uses all of it.
- **The app is width-blind** (frame F1): `OverlayModel` has no width field and `AppCtx` no columns, so per-row truncation must use a nominal budget and rely on the renderer's own column-aware `truncate_cols` clip as the backstop (`crates/renderer/src/overlay_layer.rs:264,272`).
- The panel draws the **last** `max_lines` of the composed output (`overlay_layer.rs:262-270`) — it keeps the tail. Adding five sections below the header means a short window can scroll the header off the top; frame's Known Gaps hands that decision here.
- The telemetry stores this feature reads, from `telemetry-model` D6/D11: completed-tools ring (cap 32), skills ring (16), file-activity ring (32), subagents ring (32), all newest-first; plus the **tasks keyed map** (cap 64, `taskId → {subject, status, created_ms, updated_ms}`).
- `telemetry-model` fixes several facts this layout must respect: skill entries carry **no duration** (D8 — a `Skill` tool_use→result pair is 50ms of dispatch latency, not execution); the subagent list is **flat**, with no parent linkage and therefore no tree to draw (D9); completed entries stay in their rings and the grace period for hiding them is app-side (D3/D9); and `duration_ms` is `Option`, `None` where a timestamp was missing or the interval negative (D5).
- The TS reference's per-section shapes: agents with an elapsed column and a type column padded to ≤20 (`agentsBox.ts:54-94`), tools with a status icon + name padded to 20 + right-aligned duration and a `slice(0,5)` display cap (`toolsBox.ts:19-38`), skills with a timestamp (`skillsBox.ts:20-41`), file activity with a 5-char operation label and basename-only paths (`fileActivityBox.ts:27-51`), tasks sorted newest-first with a status icon (`tasksBox.ts:19-40`). Its display caps are 5 tools / 5 skills / 10 file activities.
- The reference renders an agent **tree** with `├─`/`└─` prefixes from `parentId` (`agentsBox.ts:69-72`) — not reproducible here, since that field does not exist in real Claude Code JSONL (`telemetry-model` D9, verified across 49 `Agent` calls).
- The reference colours every status (`chalk.green('✓')` / `chalk.red('✗')`); under SD3 there is no colour, so the glyph alone must carry the distinction.
- Box drawing renders natively and cell-snapped since `d003ab2` — precisely, **157 of the 160** codepoints in `U+2500..=U+259F`, excluding the three diagonals `╱╲╳` (`crates/renderer/src/box_draw.rs:1-5`) — so `─ │ ├ └ ┌` are all safe; `✓`/`✗` (U+2713/2717) are **not** in that range and go through the normal glyph/font-fallback path.
- **The completed-tools ring holds *plain* tools only.** Per `telemetry-model` D5/D7 each completion enters the one store its `ToolKind` selects, so `Read|Write|Edit|Grep|Glob` go to file activity, `Agent` to subagents, `Skill` to skills and `TaskCreate|TaskUpdate|TaskOutput` to the tasks map — none of them reach the tools ring. A file-heavy session will therefore show a populated File Activity beside an empty Tools section, which is correct but reads as broken if the heading claims to cover all tools.
- **The panel keeps the tail**: `let start = lines.len().saturating_sub(max_lines)` (`crates/renderer/src/overlay_layer.rs:268`) — when composed output overflows, the **top** is dropped. This single fact drives B1.

## Decisions

<!-- Shared Decisions SD1–SD7 inherited verbatim from the partition manifest -->

### SD1: Stack & platform — Rust, pure wgpu/Metal, macOS ARM64 + zsh only
- **Status**: resolved
- **Rationale**: Project constant (`.claude/rules/no-gui-crates.md`, `macos-only.md`, `zsh-local-only.md`). Hand-rolled cell rendering, no GUI/widget crates; no cross-platform `#[cfg]`; no SSH/remote; no other shells. Rejected: GUI toolkits (egui/iced — defeat the manual-cell goal), cross-platform abstraction (out of scope).

### SD2: Dependency direction — `common ← term-core ← renderer ← app`, with `ai-monitor` UI/GPU-free
- **Status**: resolved
- **Rationale**: `.claude/rules/dependency-direction.md`. `crates/ai-monitor` depends on neither `renderer` nor `app` nor `wgpu`/`winit` — it stays a headless log reader so the telemetry fold is unit-testable without a display. `app` is the only crate that knows about the window. Rejected: putting the telemetry state in `app` (untestable, and duplicates what the `tabmeta` worker already reads from this crate).

### SD3: Monochrome first — the `OverlayModel` contract is byte-unchanged
- **Status**: resolved
- **Rationale**: `renderer/src/overlay_layer.rs:23` stays `OverlayModel { visible: bool, lines: Vec<String> }` with no styling. The whole dashboard is composed as plain `Vec<String>` using U+2500–259F box drawing, which the renderer already draws cell-snapped natively since `d003ab2` — so ~90% of the visual result costs zero renderer work. Colour requires extending the contract to styled spans and is explicitly a **separate later slice**, not part of this set. Rejected: adding styled spans now (widens the renderer contract before the layout is even settled); ANSI escapes inside the strings (the overlay layer draws glyphs, it does not run a VT parser).

### SD4: Policy/mechanism split — `ai-monitor` accumulates, `app` decides presentation (D21)
- **Status**: resolved
- **Rationale**: The same reasoning `left-sidebar-waiting-area` D21 applied to OSC 133 (raw events in the core, thresholds/keying in `app`): `crates/ai-monitor` exposes raw accumulated numbers and ring buffers only; **every** presentation policy lives in `crates/app` — which boxes exist, their order, truncation widths, the per-model context-window table used for context %, and any idle/grace thresholds. Precedent in-repo: the 30s waiting threshold and the `{0,1,130}` exit-code ignore-list are app-side. Rejected: a `context_percent()` on the telemetry type (bakes a model table into the headless crate, which then needs updating for every new model), formatting strings in `ai-monitor` (couples the crate to panel width).

### SD5: The watcher thread owns the state and sends snapshots
- **Status**: resolved
- **Rationale**: `LogWatcher` (`crates/ai-monitor/src/lib.rs:144`) today parses each line to an `AiEvent` and sends *that*, discarding the raw line — so there is nothing to accumulate from. The watcher thread instead owns a `SessionTelemetry`, folds each appended line into it, and sends an immutable snapshot on the existing `mpsc` channel, waking the loop via `EventLoopProxy` exactly as the PTY reader thread does. The UI thread only formats what it receives. Rejected: sending raw lines (moves JSON parsing onto the UI thread — the thing this model exists to avoid), keeping `AiEvent` on the channel and accumulating app-side (violates SD4 and re-creates the discard bug), a shared `Arc<Mutex<SessionTelemetry>>` (introduces UI-thread lock contention where a snapshot send has none).

### SD6: No new crate dependencies — `std` + `serde_json` only
- **Status**: resolved
- **Rationale**: `crates/ai-monitor` already depends on exactly `serde_json` + `notify`, and this set adds nothing to that. Event timestamps arrive in Claude Code's fixed RFC 3339 `…Z` form, so they are hand-parsed to epoch-millis inside `ai-monitor` rather than pulling a date library in for one known format. Rejected: `chrono` / `time` (a whole dependency for one fixed shape), `serde` derive structs over the log lines (the JSONL is heterogeneous and partly undocumented — `serde_json::Value` pointer lookups degrade gracefully where a derive would fail the line).

### SD7: Verification — headless unit tests + `insta` for composed lines; malformed input never panics
- **Status**: resolved
- **Rationale**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` gate every feature. The telemetry fold is asserted with plain unit tests over fixture JSONL lines checked into `crates/ai-monitor/tests/fixtures/`; the app-side line composition is snapshot-tested with `insta` (already the project's snapshot tool in `term-core`) over a hand-built telemetry snapshot, so layout changes show up as reviewable diffs. Every layer preserves the existing malformed-line contract (`crates/ai-monitor/src/lib.rs:309` — bad JSON is skipped, never panics). No GPU test is added: the renderer is untouched (SD3). Rejected: testing the dashboard through a live session log (non-deterministic), asserting rendered pixels (there is no renderer change to assert).

<!-- Feature-local decisions -->

### B1: The panel fits ~14 lines at the default window — so the summary goes **last**
- **Status**: resolved
- **Rationale**: The renderer draws the **last** `max_lines` of the composed output (`let start = lines.len().saturating_sub(max_lines)`, `overlay_layer.rs:268`), so when content overflows the **top** is lost. The principle is therefore *the least disposable content goes last* — and an earlier revision stated that principle and then violated it by putting the header first.
  **The budget, computed rather than assumed.** Default window 900×540 logical (`terminal_app.rs:230`); `ch = round(size_pt·scale·1.2)` with `size_pt: 14.0` → 34 px at 2× (`renderer/src/lib.rs:1349-1352`, `common/src/config.rs:87`); `line_h = ch·1.15 = 39.1`; `panel_h = 0.55·viewport_h = 594`; `max_lines = floor((594 − 2·10)/39.1) = ` **14**. Not ~35, which is what an earlier revision's caps composed — a 2.4× over-run that would have pushed the header and the first three sections entirely off-screen at the shipped default.
  **Order (top → bottom, clipped from the top): Tasks → Skills → Other Tools → File Activity → Agents → summary.** The frame's header block goes *last* so it always survives, and "what is running right now" sits just above it. File Activity sits **below** Other Tools deliberately: B7b's own motivating case is the file-heavy session, where Other Tools is empty while File Activity is full — with the reverse order that session would spend a guaranteed-visible slot on an empty section while decapitating the populated one above it. A summary that vanishes at the default window size is a functional failure; reading as a footer is a cosmetic one, and the trade is not close. Section order is this feature's policy under SD4, so placing frame's block is within this surface (both features compose into the same `dashboard.rs`).
  Rejected: header first (the earlier revision — guaranteed to be clipped at the default window, which its own Known Gaps then admitted); the TS reference's order (`renderer.ts:27-32`) — written for a scrollable full-screen TUI with a genuinely *pinned* header (`:60-81`), which this fixed panel cannot have; sizing everything to fit 14 lines (one row per section is too thin to be useful at the window sizes people actually work in).

### B2: Per-section row caps, with truncation stated and degradation ordered
- **Status**: resolved
- **Rationale**: Caps are **Tasks 4, Skills 3, File Activity 4, Other Tools 4, Agents 4**. With one rule line each and frame's 4-line summary block, a fully-populated dashboard composes **≈28 lines**, and an empty one **≈9** (B3 inlines empty states onto the rule).
  These caps deliberately **exceed** the 14-line default-window budget rather than fitting it, because the alternative is worse: sizing to 14 would show one row per section on the large windows where this dashboard is actually read, and the app **cannot** size adaptively — it is height-blind for the same freshness reason it is width-blind (frame F1/F4: nothing recomposes on resize). B1's ordering is what makes the overflow safe: at 14 lines a user sees Agents and the summary in full plus File Activity; the full set appears from ≈`(28·39.1 + 20)/0.55 ≈ 2000` physical px, i.e. a ~1000-logical-px-tall window, which a maximised editor-height terminal comfortably exceeds.
  The crate's larger ring caps (32/16/32/32, `telemetry-model` D6) leave headroom to raise these without touching `ai-monitor` — exactly what SD4's split is for. When a store holds more than its cap the section rule carries a `+N more` marker (B2a), so truncation is **stated, not implied**.
  Rejected: caps summing to ≤14 (unusable on a real window — and the default 900×540 is a starting size, not where work happens); the earlier revision's 6/5/4/5/6 (≈35 lines, clipping the summary and three sections at default); the reference's 5/5/10 (its 10 file activities assume a scrollable body); no truncation marker (indistinguishable from "that is all there is").

### B2a: `+N more` replaces trailing rule glyphs; it never lengthens the line
- **Status**: resolved
- **Rationale**: Frame F1 emits rules to a fixed `RULE_COLS = 104`, so the marker must be composed *into* that budget, not appended past it — appending would push the line over `RULE_COLS` and make it the first thing the renderer's `truncate_cols` clips, i.e. the marker would vanish exactly when it is needed. The rule therefore renders as `── Other Tools +7 more ─────…──`, with the marker **adjacent to the label** and the trailing `─` run shortened to keep the total at `RULE_COLS`. Placing it at the *tail* — as an earlier revision did — has the identical failure it was meant to avoid: at a 700-logical-px window `max_cols = floor((1400−20)/17) = 81`, so a marker occupying columns 82–104 is clipped away on exactly the narrow windows where truncation is most likely. Next to the label it survives any width. This is a byte-level decision because `insta` snapshots pin it (B8). Rejected: appending past `RULE_COLS` (clipped first); a marker at the rule's tail (clipped on narrow windows — the case that matters); a separate `+N more` row (costs a line from a 14-line budget to say that lines are scarce).

### B3: Empty sections render an explicit empty state, never a blank
- **Status**: resolved
- **Rationale**: A missing section and an empty one are different facts, and the goal's accept-seed requires the distinction. **The empty state is composed onto the rule line itself** — `── Skills ──────── (none yet) ──` — not onto a second row: at a 14-line budget (B1) an all-empty dashboard must not spend 10 lines saying nothing, and this halves it to 5. The wordings are `(no agents yet)` (B6), `(no tasks)`, `(no skills yet)`, `(no file activity)`, `(no other tool calls)` (B7b), following the reference's phrasing (`agentsBox.ts:59`, `toolsBox.ts:22`) though not its layout. Under SD3 there is no dim styling, so the parenthetical wording carries it.
  Tasks are additionally expected to be empty in normal use — `telemetry-model` D9a leaves the tasks map unfed if the `Task #N created successfully:` result-prose parse ever breaks — so its empty state must read as a normal condition, not an error.
  Rejected: omitting empty sections (the layout would jump as sections appear and vanish, and a reader could not tell "no data" from "not built"); a dedicated empty row (5 wasted lines out of 14); rendering `0` counts (says nothing about what the section is).

### B4: Pass/fail is two states carried by glyph alone — there is no pending state to show
- **Status**: resolved
- **Rationale**: SD3 removes colour, so `✓`/`✗` must be legible without it — they are, being distinct shapes, and are the reference's own choice. They sit at U+2713/2717, **outside** the procedurally-rendered range (157 of the 160 codepoints in `U+2500..=U+259F`, excluding `╱╲╳` — `box_draw.rs:1-5`), so they take the ordinary glyph/font-fallback path and depend on the configured font. The pair is defined once as a single constant so a font problem is a one-line change to `+`/`-` rather than a hunt through five composers.
  **Outcome is two-valued — but only for the *completed-tools* and *file-activity* rings.** An earlier revision specified a `·` pending marker for tools; that is unimplementable, because `telemetry-model` D11/R9.2 exclude the pending-**tool** map from the snapshot, so no in-flight *tool* crosses the channel. Those two rings therefore hold only finished entries and need exactly ✓/✗.
  **That reasoning does not extend to agents, and an earlier revision wrongly generalised it.** `telemetry-model` D9/R6.1 put a **not-completed** subagent entry into the subagents ring the moment an `Agent` tool_use is folded, and R6.2 completes it later — a different structure from the excluded pending-tool map. So an in-flight agent *does* reach this layer. Live-vs-completed is also a **different axis** from pass/fail: the fold records no `is_error` for agents (R6.2 marks completion with an end time only), so ✓/✗ cannot express it and reusing them would assert an outcome the data does not carry.
  **Agents therefore use a second constant pair, `●` live / `◯` completed** (U+25CF/25CB, the reference's own choice at `agentsBox.ts:74`). Both are outside the procedural range too, so they get the same single-constant-pair treatment as ✓/✗ — two pairs, four glyphs, all replaceable in one place if the font disappoints.
  Rejected: a pending marker on tool rows (no such data reaches this layer); ✓/✗ for agent liveness (conflates "still running" with "succeeded" — the fold has no agent outcome to report); colour-only status (SD3 makes it invisible); box-drawing glyphs (the range is rules, corners and blocks — no check, cross or disc); words (`ok`/`fail`/`live` cost 4-5 columns per row on a width-blind layout).

### B5: Agents render as a flat list; the reference's tree is not reproducible
- **Status**: resolved
- **Rationale**: `telemetry-model` D9 establishes there is **no parent linkage** in real Claude Code JSONL — verified across 49 `Agent` calls, whose input keys are exactly `{subagent_type, run_in_background, description, prompt, model}` — because nested agents write to a `subagents/` sibling directory that is never read. The reference's `├─`/`└─` tree (`agentsBox.ts:69-72`) would therefore render every agent at depth 0 with a misleading connector implying a hierarchy that was never observed. Rows are a flat list of `subagent_type` + description + elapsed, with a live/completed marker. Rejected: drawing the tree connectors anyway (asserts a structure the data does not contain); omitting the agents section (delegation is one of the most useful things to see).

### B6: Elapsed is frozen on completion — and the section is named for what it actually holds
- **Status**: resolved
- **Rationale**: Reusing frame F4's injected `now_ms`, a **live** subagent's elapsed is `now_ms - start`, while a **completed** one is frozen at `end - start`; otherwise a finished agent's timer keeps climbing, which the reference gets right (`logTailer.ts:71-73`) and is the single most misleading thing a status dashboard can do.
  **The section is titled "Agents", not "Active Agents", because completed entries persist indefinitely.** An earlier revision dropped the reference's 30s grace filter on the argument that "the row cap already bounds the section". That argument is wrong, and the correction matters: **the cap bounds *height*, a grace period bounds *staleness* — they are not substitutes**. On an idle session nothing new enters the subagents ring, so a finished agent is never pushed out by the cap and sits under the heading forever; and an "Active Agents" heading whose `(no active agents)` empty state becomes **unreachable after the first agent ever runs** is simply lying. Note SD4 explicitly names "any idle/grace thresholds" as app-side policy, i.e. the shared decision anticipated a threshold living exactly here.
  The resolution is to make the heading true rather than to add a second policy: the section is **"Agents"**, shows recent delegation history newest-first with a live/completed marker (B5), and its empty state reads `(no agents yet)`. A live agent is always among the newest, so it is never crowded out by history at cap 4. The same freeze rule governs a task's duration where derivable — see B6a.
  Rejected: keeping "Active Agents" with indefinite persistence (the heading contradicts the contents); a 30s grace filter (would empty the box on exactly the idle session a user is most likely staring at, and needs a threshold constant this feature can otherwise avoid); recomputing elapsed inside `ai-monitor` (`telemetry-model` D3 keeps the fold clock-free).

### B6a: Task status is rendered as its raw string; duration only for `completed`
- **Status**: resolved
- **Rationale**: Task entries are `{subject, status, created_ms, updated_ms}` (`telemetry-model` D6) — there is **no end time**, so B6's freeze rule cannot apply directly.
  **The status vocabulary is measured, not assumed.** Across every `TaskUpdate` in the corpus exactly two values occur: `completed` (87) and `in_progress` (82); `telemetry-model` D9 additionally seeds `active` at creation. **`failed` never appears** — an earlier revision invented it. Since `telemetry-model` passes the string through unmodified, an unknown value is possible in principle, so the rule is closed rather than enumerated: **`completed` is the only status treated as terminal, and every other string — known or not — is non-terminal.** A duration is shown only for a terminal task, where `updated_ms − created_ms` is correct because the last update *is* the completion.
  **The status is rendered as its raw lower-case string** (`in_progress`, `completed`), not a glyph. B4's ✓/✗ pair is a two-valued *outcome* marker and cannot express a three-plus-valued lifecycle, and inventing a third glyph for a vocabulary that may gain members is how a dashboard starts lying. A raw string is self-describing, survives an unknown value, and costs the columns it needs.
  Rejected: `completed`/`failed` as the terminal set (`failed` is unobserved and was invented); an enumerated glyph set like the reference's `◉`/`✓`/`✗` (`tasksBox.ts:13-17`) (breaks on any unlisted status); `updated_ms − created_ms` for non-terminal tasks (measures time-since-last-change, not duration).

### B6b: Rows with no duration render `—`
- **Status**: resolved
- **Rationale**: `telemetry-model` D5 makes `duration_ms` an `Option` — `None` when a timestamp was missing or the interval negative — and explicitly assigns the rendering obligation to this feature ("the app renders the reference's `—`", `toolsBox.ts:6`). The placeholder is U+2014 `—`, matching frame F3a's placeholder so the two blocks agree. This is separate from B4, which covers the status glyph, not the time column. Rejected: omitting the column (the rows would not align); `0ms` (indistinguishable from a real sub-millisecond call).

### B6c: Tasks sort by `updated_ms` descending — a map has no order
- **Status**: resolved
- **Rationale**: `telemetry-model` D6/D11 make tasks a **keyed map**, which is unordered, so "the first 4" (B2) is undefined without a sort key — and worse, iterating a `HashMap` would make B8's `insta` snapshots **flaky run to run**, since Rust's default hasher is randomly seeded per process. The key is `updated_ms` **descending** (ties broken by `created_ms` descending, then `taskId`, so the order is total and reproducible). `updated_ms` is the right key because it surfaces the task that just changed — and because D6 already evicts by it, so display order and eviction order agree instead of fighting. Rejected: `created_ms` (a task created early and updated just now would sink out of view); the reference's newest-created-first (`tasksBox.ts:21` — same defect); insertion order (a map has none); active-first (a status change would make rows jump between groups, which reads as flicker).

### B7: File paths render basename plus one parent, truncated left
- **Status**: resolved
- **Rationale**: File-activity rows are the widest content and the least useful when truncated conventionally — `/Users/hyunhokim/IdeaProjects/terminal-…` tells the reader nothing. The reference drops to the **basename** (`fileActivityBox.ts:22-26`), which loses disambiguation the moment two `mod.rs` are touched. Rows therefore render **`parent/basename`**, prefixed with `…/` when the original had more ancestors: `…/renderer/atlas.rs`.
  **The path budget is 85 columns, derived rather than asserted.** Against frame's `RULE_COLS = 104`: 2 columns of row indent + 5 for the operation label (`READ `/`WRITE`) + 1 separator + 1 status glyph + 1 separator + 8 for a right-hand duration (`1.2s`, `—`, per B6b) + 1 separator = **19** of chrome, leaving **85** for the path. (Two earlier revisions got this wrong in the same way — one stated 48, one stated 78, neither matching its own subtraction. The arithmetic is now written out term by term precisely because a bare number here is unverifiable.) When `parent/basename` exceeds 85 the parent is dropped; if the basename alone still exceeds 85 it is truncated from the **left** with a leading `…`, preserving the extension. A fixed budget is necessary because F1 leaves the app width-blind — there is no real budget to consult.
  **`Grep` and `Glob` entries carry a pattern, not a path** (`telemetry-model` D7 extracts from `file_path`/`path`/**`pattern`**). Path shortening is meaningless on a regex and actively mangles one containing `/`. Those two operations therefore render the raw pattern, right-truncated with a trailing `…`, since a pattern's discriminating part is its head. The reference has this defect and it is deliberately not inherited.
  Rejected: right-truncation for paths (clips exactly the filename, the one part that matters); basename only (cannot distinguish two `mod.rs`); full paths (one row consumes the whole width); treating patterns as paths (the reference's bug).

### B7a: Skills show a relative age, not a wall-clock time
- **Status**: resolved
- **Rationale**: `telemetry-model` D8 gives skill entries a start time and deliberately **no duration** (a `Skill` tool_use→result pair is 50 ms of dispatch latency, not execution). The reference shows an absolute `HH:MM:SS` (`skillsBox.ts:5-11`), which is **not implementable here**: frame F4 injects a bare `now_ms` epoch value, SD6 bars `chrono`/`time`, and `std` alone cannot resolve a local timezone — a UTC wall-clock would be actively confusing on a local-session dashboard. Rows therefore render the skill name and a relative age from `now_ms` (`4m ago`), which needs only subtraction. It inherits F4's staleness (frozen between snapshots) and the same `as of last update` caveat applies to the block as a whole. Rejected: absolute local time (not derivable — no timezone without a dependency); absolute UTC (misleading next to a local-session view); no time at all (the ordering would be unexplained).

### B7b: "Tools" is named for what it holds, and counts the rest
- **Status**: resolved
- **Rationale**: The completed-tools ring receives only tools that classify as *plain* — `Read|Write|Edit|Grep|Glob` go to file activity, `Agent`/`Skill`/`Task*` to their own stores (`telemetry-model` D5/D7). A heading of "Recent Tools" over a section that structurally excludes the five most common tools is misleading: on a file-heavy session it renders `(no tool calls yet)` while File Activity is full, which reads as a bug. The section is therefore titled **"Other Tools"**, making the partition visible rather than hidden, and its empty state reads `(no other tool calls)`. Rejected: "Recent Tools" (claims coverage it does not have); merging all tools into one section (loses the per-kind columns each store exists to provide, and duplicates every file row); renaming file activity instead (it is the more specific, better-named section of the two).

### B8: All five sections are `insta`-snapshot tested, including every empty state
- **Status**: resolved
- **Rationale**: Extending frame F6 rather than introducing a second mechanism. Snapshots cover: a fully-populated dashboard, an entirely empty one (every section in its empty state — the condition a fresh overlay actually shows), and one at each section's truncation boundary (`+N more`). Fixtures are hand-built `TelemetrySnapshot` values at a fixed `now_ms` so live/completed elapsed values are deterministic. Rejected: testing only the populated case (the empty dashboard is what a user sees first, and is the most likely thing to look broken); driving the composer from a real log (non-deterministic, and `telemetry-model` R10.1 already forbids reading outside the fixtures dir).

## Constraints
- This feature owns **only** `crates/app/src/dashboard.rs` — `overlay.rs`, `main.rs`, `seam.rs`, `crates/renderer/**` and `crates/ai-monitor/**` are all outside the declared surface
- `dashboard.rs` remains a single file (bare-path glob), and its composer stays pure — no clock read, no filesystem, no globals
- `OverlayModel` stays `{ visible, lines }` (SD3); sections stay open-right (frame F1) since the app is still width-blind
- No new runtime dependency; `insta` is already a dev-dependency from frame F6
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` stays clean
- Section row caps and order are app-side policy (SD4) — no change to `ai-monitor`'s ring caps is permitted from here

## Requirements

### R0: Five sections compose below the header (Confirmed Goal, accept-seed)

#### R0.1: A fully-populated dashboard matches its pinned form
- **Given**: A hand-built snapshot populating all five stores past their caps, at a fixed `now_ms`
- **When**: The composer runs
- **Then**: The output is the five sections in B1's order followed by the frame's summary block, ≈28 lines, pinned by an `insta` snapshot

#### R0.2: An entirely empty dashboard matches its pinned form
- **Given**: A snapshot with every store empty
- **When**: The composer runs
- **Then**: The output is 5 inline empty rules plus the 4-line summary — 9 lines — pinned by an `insta` snapshot

### R1: Section order and placement (B1)

#### R1.1: The order is fixed
- **Given**: Any snapshot
- **When**: The composer runs
- **Then**: Sections appear as Tasks → Skills → Other Tools → File Activity → Agents, with the frame's summary block **last**

#### R1.2: The summary survives a 14-line panel
- **Given**: A populated dashboard of ≈28 composed lines and a panel budget of 14
- **When**: The renderer keeps the last 14
- **Then**: The complete summary block is among them — it is never the content that gets clipped

### R2: Row caps and truncation marking (B2, B2a)

#### R2.1: Each section renders at most its cap
- **Given**: A snapshot whose stores each hold more entries than their caps
- **When**: The composer runs
- **Then**: Tasks shows 4, Skills 3, Other Tools 4, File Activity 4, Agents 4 rows

#### R2.2: Truncation is stated on the rule, next to the label
- **Given**: An Other Tools ring holding 11 entries against a cap of 4
- **When**: The section is composed
- **Then**: Its rule reads `── Other Tools +7 more ───…`, with the marker adjacent to the label and the total still `RULE_COLS`

#### R2.3: A section at or below its cap carries no marker
- **Given**: A store holding exactly its cap
- **When**: The section is composed
- **Then**: The rule carries no `+N more`

### R3: Empty states (B3)

#### R3.1: An empty section is one line, inline on the rule
- **Given**: An empty skills ring
- **When**: The section is composed
- **Then**: It emits exactly one line — `── Skills ──… (no skills yet) ──` — not a rule plus a placeholder row

#### R3.2: Each section has its own wording
- **Given**: All five stores empty
- **When**: The composer runs
- **Then**: The five rules read `(no tasks)`, `(no skills yet)`, `(no other tool calls)`, `(no file activity)`, `(no agents yet)`

### R4: Status and liveness markers (B4)

#### R4.1: Tool and file outcomes use the ✓/✗ pair
- **Given**: One successful and one failed completed entry
- **When**: Their rows are composed
- **Then**: They carry `✓` and `✗` from a single shared constant pair

#### R4.2: Agents use the live/completed pair, not the outcome pair
- **Given**: One in-flight and one completed subagent
- **When**: Their rows are composed
- **Then**: They carry `●` and `◯` from a second shared constant pair — never `✓`/`✗`, which would assert an outcome the fold does not record

#### R4.3: Tool rows have no pending state
- **Given**: Any snapshot
- **When**: Tool and file rows are composed
- **Then**: Only two outcome states occur — no pending marker, since no in-flight tool reaches this layer

### R5: Agents (B5, B6)

#### R5.1: Agents render flat with no tree connectors
- **Given**: Three subagents in the ring
- **When**: The section is composed
- **Then**: All three are rendered at the same indent with no `├─`/`└─` prefix

#### R5.2: A live agent's elapsed grows; a completed one is frozen
- **Given**: One subagent started 90s before `now_ms` and still running, and one that ran 30s and completed
- **When**: The rows are composed
- **Then**: The first shows `1m30s` (from `now_ms`) and the second `30s` (from its own end time), which does not change as `now_ms` advances

#### R5.3: The heading and empty state match the contents
- **Given**: The agents section
- **When**: It is composed
- **Then**: Its label is `Agents` and its empty state `(no agents yet)` — neither claims the contents are exclusively active

#### R5.4: Rows carry type, description and model
- **Given**: A subagent with `subagent_type: "general-purpose"`, a description, and `model: "opus"`
- **When**: Its row is composed
- **Then**: All three appear, truncated to fit

### R6: Tasks (B6a, B6c)

#### R6.1: Tasks sort by `updated_ms` descending, deterministically
- **Given**: Six tasks with distinct `updated_ms`, two sharing one
- **When**: The section is composed twice in separate processes
- **Then**: Both runs emit the same 4 rows in the same order — ties broken by `created_ms` then `taskId`, so no `HashMap` iteration order leaks

#### R6.2: Status renders as its raw string
- **Given**: Tasks with statuses `in_progress`, `completed` and an unrecognised `blocked`
- **When**: Their rows are composed
- **Then**: Each shows its own string verbatim, with no glyph substitution and no panic on the unknown value

#### R6.3: Only a `completed` task shows a duration
- **Given**: One `completed` task and one `in_progress` task, both with `created_ms` and `updated_ms`
- **When**: Their rows are composed
- **Then**: The completed one shows `updated_ms − created_ms`; the other shows no duration

### R7: Skills, tools and file activity rows (B6b, B7, B7a, B7b)

#### R7.1: Skills show a relative age and no duration
- **Given**: A skill entry started 4 minutes before `now_ms`
- **When**: Its row is composed
- **Then**: It shows the skill name and `4m ago` — no duration, since the tool_use→result pair measures dispatch latency

#### R7.2: A missing duration renders `—`
- **Given**: A completed tool whose `duration_ms` is `None`
- **When**: Its row is composed
- **Then**: The duration column shows `—`, not `0ms` and not a blank

#### R7.3: Paths keep the filename and one parent
- **Given**: A file activity on `/Users/x/proj/crates/renderer/atlas.rs`
- **When**: Its row is composed
- **Then**: It shows `…/renderer/atlas.rs`, within an 85-column path budget

#### R7.4: An over-long basename truncates from the left
- **Given**: A basename longer than the path budget
- **When**: Its row is composed
- **Then**: It is truncated from the left with a leading `…`, preserving the extension

#### R7.5: Grep and Glob render their pattern, not a path
- **Given**: A `Grep` entry whose captured string is the pattern `src/.*/mod\.rs`
- **When**: Its row is composed
- **Then**: The pattern is shown as-is, right-truncated if needed — never basename-shortened

#### R7.6: The tools section is named for what it holds
- **Given**: A snapshot whose file-activity ring is full and whose completed-tools ring is empty
- **When**: The composer runs
- **Then**: The section reads `── Other Tools ──… (no other tool calls) ──`, so the empty section is not mistaken for a failure to record tools

### R8: Verification (B8)

#### R8.1: Every section and empty state is snapshot-pinned
- **Given**: The test suite
- **When**: `cargo test -p app` runs
- **Then**: `insta` snapshots cover the populated dashboard, the fully-empty dashboard, and each section at its truncation boundary

#### R8.2: Snapshots are deterministic and offline
- **Given**: The tests
- **When**: They run repeatedly on a machine with no `~/.claude`
- **Then**: They pass identically every time — all inputs are hand-built at a fixed `now_ms`, with no live log and no clock read

#### R8.3: Workspace gates stay green
- **Given**: The workspace after this feature
- **When**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` run
- **Then**: Both pass, and the diff touches only `crates/app/src/dashboard.rs`

## Tasks

### T1: Section scaffolding — order, caps, empty states, truncation marker [infra]
- **Fulfills**: R1, R2, R3
- **Depends on**: (none)
- Extend the frame's composer with the five sections in B1's order and the summary last; add the per-section cap constants, the inline empty-state rule form, and the `+N more` marker placed adjacent to the label within `RULE_COLS`.

### T2: Status and liveness marker constants [infra]
- **Fulfills**: R4
- **Depends on**: T1
- Define the two shared constant pairs — `✓`/`✗` for outcome, `●`/`◯` for agent liveness — in one place each, and apply them per section.

### T3: Agents section [vertical]
- **Fulfills**: R5
- **Depends on**: T2
- Flat rows with type, description and model; elapsed from `now_ms` when live and frozen at the end time when completed; `Agents` heading and `(no agents yet)` empty state.

### T4: Tasks section [vertical]
- **Fulfills**: R6
- **Depends on**: T2
- Total ordering by `updated_ms` desc then `created_ms` then `taskId`; raw status strings with an unknown-value fallback; duration only for `completed`.

### T5: Skills, Other Tools and File Activity sections [vertical]
- **Fulfills**: R7
- **Depends on**: T2
- Relative skill age from `now_ms`; `—` for a missing duration; the 85-column path budget with parent+basename and left-truncation; the separate pattern rule for `Grep`/`Glob`; the `Other Tools` heading.

### T6: Snapshot tests and gates [infra]
- **Fulfills**: R0, R8
- **Depends on**: T3, T4, T5
- Hand-build `TelemetrySnapshot` fixtures at a fixed `now_ms` covering populated, empty and at-cap cases; pin them with `insta`; run the workspace gates.

## External Dependencies

### Pre-work
- `overlay-dashboard-frame` must be merged first — this feature extends its composer, `RULE_COLS`, injected clock and `insta` dev-dependency (the `depends_on` edge).

### Post-work
- Eyeball via `/run-term` at more than one window size. The row caps (B2) and the panel fraction are the constants most likely to move; the caps are one-line changes, the panel fraction is out of surface.

## Known Gaps
- **The topmost visible section is routinely headless.** Clipping cuts mid-composition, not at a section boundary, so whichever section straddles the cut shows its rows with its `── Label ──` rule scrolled off above them. On a file-heavy session at the default window that is File Activity: four paths with no heading. Harmless once a reader knows the layout, confusing on first sight, and unfixable width- and height-blind — a section that knew the remaining budget could drop itself rather than be decapitated.
- **At the default 900×540 window only ~14 of the ~28 composed lines are visible** (B1/B2). This is a deliberate, ordered trade — Agents and the summary survive, Tasks and Skills clip first — not an oversight, but it does mean the dashboard is designed for a taller window (~1000 logical px) than the app opens with. Whether the default window, the 0.55 panel fraction (`overlay_layer.rs:299`) or the caps should move is a judgement best made after `/run-term`; the first two are outside this feature's surface.
- **The app cannot size sections adaptively.** `viewport_h`, `cell` and `scale` *are* readable from the sidebar's published `SidebarGeom`, so a line budget is technically derivable — but nothing recomposes the overlay on resize (frame F4), so a computed budget would be stale from the first window drag. Adaptive sizing needs a periodic or resize-triggered overlay poll in `terminal_app.rs`, which is out of surface for the whole set.
- **Row caps are reasoned, not eyeballed.** The arithmetic is now explicit, but nobody has seen six sections in the real panel. The `+N more` marker makes over-tight caps visible and the caps are one-line constants.
- **Tasks may be permanently empty.** `telemetry-model` D9a flags the `Task #N created successfully:` result-prose parse as the one format-fragile step; if it breaks, this section renders its empty state forever and only the `unparsed_task_creates` diagnostic reveals why. That counter is not currently surfaced in the layout.
- **`✓`/`✗` depend on font coverage** (B4) — they are outside the procedurally-rendered U+2500–259F range. Untested against the configured font; the constant pair exists so the fallback is a one-line change.
- **The summary block survives clipping only because it is composed last** (B1). That is a layout workaround, not a pinned header: a genuinely pinned header needs a renderer-side change to draw it outside the scrolled region, which is out of surface and deferred with the colour slice.
