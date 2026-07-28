# partition-manifest.md — ai-status-dashboard
- generated_at: 2026-07-28
- goal: Deepen the Cmd+Shift+M ai-monitor overlay from a flat `AiEvent` feed into a real session status dashboard — token counts, context %, uptime/idle, active subagents, recent tools with pass/fail, skills, file activity, tasks — modelled on the TypeScript ai-monitor at `/Users/hyunhokim/IdeaProjects/ai-monitor` (`src/parser/logTailer.ts` + `src/ui/`), split across `crates/ai-monitor` (stateful telemetry fold) and `crates/app/src/overlay.rs` (layout policy), with the renderer unchanged.

## Shared Decisions

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

## Features  (ordered — a dependency appears before its dependents)

### telemetry-model
- declared-surface:
  - crates/ai-monitor/src/telemetry.rs
  - crates/ai-monitor/src/lib.rs
  - crates/ai-monitor/tests/**
  - crates/ai-monitor/Cargo.toml
- depends_on: []
- accept-seed: `SessionTelemetry::apply(&mut self, line: &str)` folds a fixture JSONL transcript into correct cumulative token totals (input/output/cache-read/cache-creation), first/last event times, and the tool/skill/file/task/subagent ring buffers, with each `tool_use.id` cleared by its matching `tool_result` and errors recorded as failures — all asserted headlessly, and malformed lines skipped without panicking.
- pre-approved-batch: yes

### telemetry-watcher
- declared-surface:
  - crates/ai-monitor/src/lib.rs
  - crates/app/src/overlay.rs
- depends_on: [telemetry-model]
- accept-seed: Cmd+Shift+M still opens a live-updating overlay that backfills the existing tail and then tracks appends — now because the watcher thread folds every line into its own `SessionTelemetry` and sends immutable snapshots, with no JSON parsing left on the UI thread.
- pre-approved-batch: yes

### overlay-dashboard-frame
- declared-surface:
  - crates/app/src/dashboard.rs
  - crates/app/src/overlay.rs
  - crates/app/src/main.rs
  - crates/app/Cargo.toml
- depends_on: [telemetry-watcher]
- accept-seed: The overlay renders a box-drawn header from a telemetry snapshot — session/model, input/output/cache tokens, context % against the app-side per-model context-window table, and uptime/idle — fitted to the panel's column budget without wrapping or overflowing.
- pre-approved-batch: yes

### overlay-dashboard-boxes
- declared-surface:
  - crates/app/src/dashboard.rs
- depends_on: [overlay-dashboard-frame]
- accept-seed: Beneath the header the overlay renders box-drawn Tasks, Active Agents, Skills, Recent Tools (pass/fail + duration) and File Activity sections, each truncated to the panel width and showing an explicit empty-state line when it has nothing to report.
- pre-approved-batch: yes
