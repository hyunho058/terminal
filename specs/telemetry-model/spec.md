# Spec: telemetry-model

## Meta
- **Created**: 2026-07-28
- **Type**: dev
- **Status**: ready
- **Set**: ai-status-dashboard (`specs/ai-status-dashboard/partition-manifest.md`)
- **Mode**: batch (`pre-approved-batch: yes`) — the L0/L2/L3/L4 human prompts were skipped under decompose's single partition gate; the L2-reviewer, coverage, GWT and DAG gates all ran.
- **Approved by**: decompose partition gate (`specs/ai-status-dashboard/partition-manifest.md`)
- **Approved at**: 2026-07-28

## Declared Surface
- feature-id: telemetry-model
- depends_on: []
- declared-surface:
  - crates/ai-monitor/src/telemetry.rs
  - crates/ai-monitor/src/lib.rs
  - crates/ai-monitor/tests/**
  - crates/ai-monitor/Cargo.toml

## Goal
Add a stateful `SessionTelemetry` to `crates/ai-monitor` with `apply(&mut self, line: &str)` that folds one Claude Code JSONL line into accumulated session telemetry: token totals from `message.usage` (`input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`), first/last event time, a pending-tool map keyed by `tool_use.id` and cleared on the matching `tool_result`, plus ring buffers for completed tools, skills, file activity, tasks, and active subagents. Pure over its input, unit-tested against fixture lines.

> As stated (and as worded in the approved manifest's accept-seed) this asks for a **ring buffer** for tasks. L1 measurement showed that to be the wrong structure — see the Confirmed Goal note and D6/D9. Everything else stands as asked.

## Non-goals
- Any presentation: no formatted strings, no context %, no per-model context-window table, no uptime/idle numbers, no truncation (all app-side per SD4)
- File I/O, watching, or threading — this feature folds strings; `telemetry-watcher` owns the tail loop and the channel
- Gemini CLI log support (`geminiLogTailer.ts` is not mirrored; Claude Code JSONL only)
- Changing or removing the existing `AiEvent` / `parse_line` / `backfill` / `activity` surfaces that `tabmeta` depends on
- Persistence or historical analytics beyond what the live log file contains

## Confirmed Goal
`crates/ai-monitor` gains a new `telemetry` module exposing `SessionTelemetry` — a bounded, headless accumulator whose `apply(&mut self, line: &str)` folds one JSONL line, and whose `snapshot()` yields an owned `TelemetrySnapshot` carrying raw counters, activity ring buffers (tools, skills, file activity, subagents), a keyed tasks map, and diagnostic counters. It is pure over its input (no clock, no filesystem), never panics on malformed input, is bounded in memory, and is asserted headlessly against fixture JSONL lines checked into `crates/ai-monitor/tests/fixtures/`. It contains no presentation policy.

> Note on the manifest's accept-seed, which says "ring buffers for … tasks": tasks are a **keyed map**, not a ring. `TaskUpdate` mutates an entry created earlier by `taskId` (192 updates vs 86 creates measured), which a newest-first ring cannot represent. Same data, corrected structure — see D6/D9.

## Research

### The crate as it stands
- `crates/ai-monitor` depends on exactly `serde_json` + `notify` — no serde derive, no date library (`crates/ai-monitor/Cargo.toml:8-10`).
- `parse_line` collapses every line to `AiEvent { role, summary }` and **discards the rest**; `LogWatcher` sends that `AiEvent` and drops the raw line, so today there is nothing to accumulate from (`crates/ai-monitor/src/lib.rs:77-91`, `:199-206`). This is the motivating gap.
- The malformed-line contract is established and tested: bad JSON is skipped, never panics (`crates/ai-monitor/src/lib.rs:309-319`).
- `activity.rs` is the precedent sibling module for "additive, std + serde_json only, changes no existing function, leaves `Cargo.toml` untouched" (`crates/ai-monitor/src/activity.rs:1-8`), and it reads the heterogeneous JSONL exclusively through `serde_json::Value` pointer lookups with typed fallbacks (`:150-190`).
- `activity.rs` already handles Claude Code's slash-command wrapper turns and documents from real data that the wrapper text arrives inside the `message.content[]` **array** form, not only as a plain string (`crates/ai-monitor/src/activity.rs:167-170`, test at `:406-423`).
- Tests are inline `#[cfg(test)] mod tests` with a `tmpfile` helper; `term-core` is where `insta` lives, `ai-monitor` uses plain assertions (`crates/ai-monitor/src/activity.rs:256-267`).
- Project rules: no `unwrap()` outside tests, `thiserror` per-crate errors, `cargo clippy --workspace --all-targets -- -D warnings` (`CLAUDE.md`).

### The TypeScript reference (`/Users/hyunhokim/IdeaProjects/ai-monitor`)
- `LogTailer.parseLine` is the fold being ported: usage accounting, `firstEventTime`/`lastEventTime`, `allPendingTools` keyed by tool id, `agents`/`tasks` maps, and `recentTools`/`recentSkills`/`fileActivities` ring buffers (`src/parser/logTailer.ts:224-433`).
- It reads `usage` from `event.usage` or `event.message.usage` (`:497-508`) and `model` from `event.model` or `event.message.model` (`:488-495`).
- It correlates tools both as top-level `type: "tool_use"`/`"tool_result"` events and as blocks nested in `message.content[]` (`:267-304`).
- It classifies tools into agent / skill / file, and maps `Read|Write|Edit|Grep|Glob` to read/write/grep/list operations (`:436-453`).
- Display caps: 5 skills, 10 recent tools, 10 file activities (`:300`, `:406`, `:416`).
- Grace-period filtering of completed agents/tasks (30s) and uptime/idle happen in `getState()` against `Date.now()`, i.e. **at read time, not in the fold** (`:64-115`).

### Real-log grounding

Sampled `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal-feat-ai-status-overlay/5e59b629-….jsonl`, **first 158 lines**.

> **Caveat on the numbers.** That log is the *live session in which this spec was written*, so it grows while being measured and aggregate counts are **not reproducible** — two passes minutes apart legitimately disagree (43,858 vs 45,087 naive output tokens). What is stable, and what the decisions rest on, is (a) the **ratio**, (b) the **per-message example**, which reproduces exactly, and (c) the **structural** observations. Any test must assert against a fixture checked into `crates/ai-monitor/tests/fixtures/`, never against a live log.

- **`message.usage` is replicated across split lines — the single most consequential finding.** Claude Code writes one JSONL line per assistant content block, all sharing one `message.id` and `requestId`, each carrying a **byte-identical** `usage` object. Reproduces exactly: `msg_011CdU41y6K75xryUkVu2zED` occupies 4 lines (`thinking`, `text`, `tool_use`, `tool_use`), every one reporting `in=2, out=441, cache_read=111577, cache_creation=3355`. Naive per-line summation (what the TS reference does at `logTailer.ts:260-264`) over-reports by **≈2.3×** — 43,858 output tokens against 18,604 deduplicated over the sampled window; input 99 vs 41. The over-report grows with reasoning-heavy turns, which emit more blocks per message.
- Replicas are **contiguous among usage-bearing lines**: over the sampled window, zero `message.id` groups are non-contiguous when counted across assistant lines. Non-assistant lines (`attachment`, `mode`, …) do interleave in raw file order but carry no `message.id`, so they cannot disturb a seen-set keyed on it.
- Line `type` values: `assistant`(53), `attachment`(38), `user`(32), `permission-mode`(11), `mode`(11), `last-prompt`(11), `file-history-snapshot`(1), `file-history-delta`(1) — heterogeneous, several undocumented.
- `timestamp` is present as RFC 3339 `2026-07-28T07:17:00.461Z` on every `assistant`/`user`/`attachment` line, and **absent (null)** on `mode` / `permission-mode` / `last-prompt` / `file-history-snapshot` bookkeeping lines.
- Observed `usage` keys: `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`, `cache_creation`, `inference_geo`, `iterations`, `server_tool_use`, `service_tier`, `speed`. **No thinking/thought-token field** — the TS state's `thoughtTokens` has no Claude Code source.
- **Cumulative cache-read dwarfs live context.** Cache-read is re-billed in full every turn: cumulative ≈1.9M against a live context of ≈117k on the same window — a ≈16× gap, *seven times larger than the 2.3× the dedupe fixes*. This is why the cumulative totals and the context gauge are different quantities (D2), not two views of one.
- Content blocks on assistant lines: `tool_use`(≈29), `thinking`(15), `text`(9). On user lines: `tool_result`(≈29), `text`(2), plus one plain-string content.
- `tool_result.is_error` distribution over the sampled window: absent/`null` 13, `false` 15, **`true` 1**. Successes routinely omit the field, so **absent must read as success** and only an explicit `true` is a failure.
- Assistant lines carry `attributionSkill` (e.g. `harness-ops:decompose`, `harness-ops:specify`) **ambiently — on every line of that skill's execution**: 41 lines for one `/decompose`, and the `/specify` count keeps climbing as that run proceeds. It is attribution, not an invocation event.
- The `<command-name>/harness-ops:decompose</command-name>` wrapper appeared in the plain-**string** content form here; `activity.rs:406-423` documents the **array**-text form from other real data. Both forms occur.
- **Subagent transcripts are written to a sibling directory, not the session file.** Verified: `<session-uuid>/subagents/agent-<id>.jsonl` (plus an `agent-<id>.meta.json`), whose lines carry `isSidechain: true` and their **own** `message.usage`. `newest_jsonl` (`crates/ai-monitor/src/lib.rs:56-72`) reads only `*.jsonl` directly in the project dir and is non-recursive, so it never sees them. In the parent session file `isSidechain` is uniformly `false`.
- Consequently there is **no observable parent/child linkage between `Agent` calls in the session file** — the TS reference's `input.parentId` / `input.parent_id` (`logTailer.ts:336-337`) has no counterpart in real Claude Code JSONL, so its agent *tree* cannot be reconstructed from the parent transcript.
- `model` is `claude-opus-5` — a value absent from the TS reference's `MODEL_LIMITS` table (which stops at `claude-opus-4`). Relevant to the downstream `overlay-dashboard-frame`, not to this feature.

### Corpus-wide grounding (60 project logs under `~/.claude/projects`, subagent files excluded)

Widened from the single session because three decisions turned on frequencies a 158-line window cannot establish.

- **Message-identity coverage** over 5,715 usage-bearing lines: `message.id` absent **0**; `requestId` absent **2**; both absent **0**; a `message.id` appearing under more than one `requestId` (a real retry) **0**. This is what makes D1's composite key safe rather than speculative.
- **Task tooling — measured, and `TodoWrite` does not exist here.** `TaskUpdate` 192 (`{taskId,status}` 165, `{taskId,addBlockedBy}` 22, `{taskId,description}` 4, `{taskId,status,description}` 1), `TaskCreate` 86 (`{subject,description,activeForm}` 85, batch `{tasks}` 1), `TaskOutput` 8 (`{task_id,block,timeout}`), **`TodoWrite` 0**. `taskId` is a **string** in all 192 updates.
- **`TaskCreate` does not return its id in the input.** The assigned id appears only in the `tool_result` content as `Task #N created successfully: <subject>` — verified on three separate sessions. Pairing create-input to create-result by `tool_use_id` is therefore the only way to build a `taskId → subject` map, and updates (192) outnumber creates (86) more than 2:1, so tasks must be a keyed map, not an append ring.
- **`Agent` input keys** across 49 calls: exactly `{subagent_type, run_in_background, description, prompt, model}` — **no `parentId`/`parent_id`**, confirming the reference's agent tree is not reconstructable here. `model` present on 16 of 49.
- **The two skill signals are disjoint, not overlapping.** In the sampled window the `/decompose` `<command-name>` wrapper (line 5, `07:17:56Z`) has no corresponding `Skill` tool_use, and the only `Skill` tool_use (line 124, `07:22:00Z`, `harness-ops:specify`) has no wrapper. The cross-source collision the reference's 10s dedupe window suppresses occurs **zero** times.
- **A `Skill` tool_use → `tool_result` pair spans 50 ms** (`07:22:00.186Z` → `07:22:00.236Z`) — instruction-load latency. The skill it launched then ran for minutes across later turns, so that pair is not a run duration.

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

### D1: Usage is folded once per assistant message — deduplicated by the composite key `(message.id, requestId)`
- **Status**: resolved
- **Rationale**: Claude Code splits one assistant API response across N JSONL lines (one per content block) that all share a `message.id` / `requestId` and each replicate a **byte-identical** `message.usage` — so naive per-line summation over-reports by ≈2.3× (see Research). `apply` folds a line's `usage` only the first time it sees that line's identity.
  **The key is the composite `(message.id, requestId)`, not a fallback chain.** A retried or resumed request can legitimately re-emit the same `message.id` under a *new* `requestId` with genuinely new billing; keying on `message.id` alone would silently drop it — trading a loud 2.3× over-count for an invisible under-count. The composite keeps the replica-collapse (both fields are constant across a split) while letting a genuine retry through. When either field is absent the fold falls back to the other, then to the line's `uuid` (always present, never shared), which degrades to per-line counting rather than to silent loss.
  **Later replica differs from the folded one** → ignore it and increment a `usage_conflicts` counter. Replicas were byte-identical across every usage-bearing message in the sample, so a difference means the assumption broke; counting it makes that visible instead of letting it corrupt the totals silently.
  **An evicted key that reappears** (see D6's cap) is folded again — double-counting that message. This is the accepted failure mode of a bounded set, and D6 sizes the cap against the measured contiguity so it cannot be reached in practice.
  **Verified at scale** over 5,715 usage-bearing lines across 60 project logs: `message.id` absent **0**, `requestId` absent **2**, both absent **0**, and `message.id` observed with more than one `requestId` (i.e. an actual retry) **0**. So the composite collapses exactly what a `message.id`-only key would, the `requestId`-absent path degrades to `message.id` alone (which still collapses replicas — no 2.3× regression), and the `uuid` path — which **is** the 2.3× bug, since per-line counting is precisely the defect — is unreachable on all observed data.
  Rejected: summing every line (the TS behaviour at `logTailer.ts:260-264` — measurably wrong); `message.id` alone (drops retries, per the steelman above); `requestId` alone (absent on 2/5,715); last-write-wins (same result, needs the previous value rewound); an unbounded seen-set (D6 — unbounded memory in a long-lived terminal).

### D2: `context_used` is a latest-value gauge; the four totals are cumulative **cost** counters
- **Status**: resolved
- **Rationale**: Two different quantities the TS reference correctly separates (`logTailer.ts:250-264`) and that must never be conflated.
  **The gauge.** `context_used = input_tokens + cache_read_input_tokens + cache_creation_input_tokens` of the **most recently folded** assistant usage — "most recent" meaning *last in file order as folded*, not max-timestamp, since the fold is a single forward pass and a JSONL log is append-ordered. This is live context occupancy; the app divides it by the model's window for context % (SD4).
  **The totals.** `total_input/output/cache_read/cache_creation` are cumulative across deduplicated messages and are **cost accounting, not occupancy**. Cache-read is re-billed in full every turn, so cumulative cache-read reached ≈1.9M against a live context of ≈117k on the sampled window — a ≈16× gap. `overlay-dashboard-frame` must therefore label them as spend ("cache 1.9M" = tokens billed), never as "context used"; presenting a cumulative total next to a percentage would read as though the session were 16× over its window.
  Rejected: summing for the gauge (superlinear and meaningless); exposing only a percentage (SD4 — the model table would move into the crate); folding the gauge from *every* replica line last-wins like the reference (identical result given byte-identical replicas, but it re-reads state D1 has already decided to skip).

### D3: `apply` is pure over its input — no clock, no filesystem, no I/O
- **Status**: resolved
- **Rationale**: The signature is exactly `apply(&mut self, line: &str)`; the only inputs are `self` and the line. Uptime and idle are consequently **not fields** — the crate exposes `first_event_ms` / `last_event_ms` and the app subtracts its own `now`. Likewise the TS reference's 30s grace-period filtering of completed agents/tasks (`logTailer.ts:64-92`) is a read-time policy and stays app-side (SD4). This is what makes the fixture tests deterministic rather than time-dependent. Rejected: storing `uptime_ms`/`idle_ms` like the TS `getState()` (every value becomes clock-dependent and the fixture assertions become flaky); taking a `now: u64` parameter (pushes a clock into the fold's signature for two subtractions the app can do itself).

### D4: Timestamps are hand-parsed RFC 3339; a line without one does not move the clock
- **Status**: resolved
- **Rationale**: `timestamp` is present in the fixed `YYYY-MM-DDTHH:MM:SS.sssZ` form on every `assistant`/`user`/`attachment` line and **absent on bookkeeping lines** (`mode`, `permission-mode`, `last-prompt`, `file-history-snapshot`) — all verified on a real session. A fixed-shape parse to epoch-millis using `std` only satisfies SD6. A line whose timestamp is missing or unparseable still folds its other content but leaves `first_event_ms` / `last_event_ms` untouched. Rejected: the TS reference's `?? Date.now()` fallback (`logTailer.ts:233`) — it fabricates activity, so a `permission-mode` bookkeeping line would reset idle to zero and make a long-idle session render as live, which is exactly the signal the dashboard exists to show; `chrono`/`time` (SD6).

### D5: Tools correlate via a pending map keyed by `tool_use.id`, read from both the nested and top-level forms
- **Status**: resolved
- **Rationale**: A `tool_use` opens a pending entry keyed by its `id`, holding the name, start time, `ToolKind` and the per-kind facts D7 extracts; the matching `tool_result.tool_use_id` closes it and pushes into the ring D7 selects. Blocks are collected from **both** the nested form (`message.content[]` — `tool_use` on assistant lines, `tool_result` on user lines: the form real logs actually use, ≈29 each in the sample) and the top-level `type: "tool_use"` / `"tool_result"` events the TS reference also handles (`logTailer.ts:267-304`).
  **Outcome:** only an explicit `is_error == true` is a failure; absent, `null` or `false` is success. Verified distribution over the sampled window: absent 13 / `false` 15 / `true` 1 — successes routinely omit the field, so an absent value must not read as unknown.
  **The four edge cases, all reachable on day one:**
  - **Orphan `tool_result`** (no pending entry) — *guaranteed*, not hypothetical: `telemetry-watcher` starts from a bounded tail, so results whose `tool_use` sits above the window always occur. Ignore it, increment `orphan_tool_results` (D11); never fabricate an entry.
  - **Duration when either side lacks a parseable timestamp** (D4 permits such lines) — record the completion with `duration_ms: None`. The app renders the TS reference's `—` (`toolsBox.ts:6`).
  - **Negative duration** (result timestamp earlier than its `tool_use`) — clamp to `None`, not to zero: zero would render as a real "0ms" measurement.
  - **The same `tool_use.id` opened twice** — the later `tool_use` replaces the pending entry (last-wins) and increments `duplicate_tool_ids`; the alternative, keeping the first, would attribute the result to a stale start time.
  Rejected: nested-only (blind if the writer emits top-level events); treating absent `is_error` as unknown (most real successes omit it — the dashboard would read "unknown" almost everywhere); dropping orphan results silently (hides a mis-sized tail window).

### D6: Every accumulating structure is bounded — but ring eviction and dedupe eviction are different risks
- **Status**: resolved
- **Rationale**: This state lives for the whole life of a long-running terminal, so nothing may grow without limit. The two kinds of bound have **different failure consequences and must not be sized alike**: evicting from a display ring merely drops a row the user was not going to see; evicting from the D1 dedupe set silently **double-counts tokens**, corrupting the headline number.
  - **Display rings** (newest-first): completed tools 32, skills 16, file activity 32, subagents 32 — ≈3× the TS display counts (5 skills / 10 tools / 10 files at `logTailer.ts:300,406,416`) so the app can change how much it shows without a crate change (SD4).
  - **Tasks are a keyed map, not a ring** — cap 64, evicted **oldest-updated**. A ring is the wrong structure: `TaskUpdate` carries `{taskId, status}` and **no subject**, so it is an in-place mutation of an entry created earlier (measured: 192 updates against 86 creates). A newest-first ring cannot express "task 3 moved to in_progress"; it would append an orphan row with no description. The map is `taskId → { subject, status, created_ms, updated_ms }`, keyed by the **string** `taskId` (D9). Eviction is by `updated_ms`, not `created_ms`: updates cluster late (dependency wiring arrives after the plan is laid out), so oldest-*created* would evict the long-running task that is still most active — precisely backwards. The field is already stored, so this costs nothing. Headroom is large regardless: the largest observed `TaskCreate` count in any single session is 15, against a cap of 64.
  - **Pending-tool map: cap 64**, evicted oldest-first. A `tool_use` whose result never arrives (session ends mid-tool, result outside the tailed window) must not leak. 64 is well above any observed in-flight count — parallel tool batches in the sample peaked at 2.
  - **D1 dedupe set: cap 64**, evicted oldest-first, sized against measured contiguity: replicas of one `message.id` are contiguous among usage-bearing lines (zero non-contiguous groups in the sample) and the widest observed run is 4 lines, so 64 leaves ≈16× headroom. Reaching it requires 64 distinct messages interleaved mid-split — a writer behaviour never observed. This cap is what D1's correctness rests on; it is stated numerically here so it can be asserted in a test rather than assumed.
  Rejected: unbounded `Vec`s / `HashMap`s (unbounded memory in a process that must stay a terminal); one shared cap for rings and dedupe (conflates a cosmetic loss with a correctness loss); caps equal to the display counts (couples the crate to layout).

### D7: The crate classifies tools into a `ToolKind` enum; labels, icons and ordering are app-side
- **Status**: resolved
- **Rationale**: The crate must branch on tool name anyway to know which store a completed tool belongs in, so classification is mechanism, not policy. The variants are `Agent` → subagent, `Skill` → skill, `Read|Write|Edit|Grep|Glob` → file activity with a read/write/grep/list operation, **`TaskCreate|TaskUpdate|TaskOutput` → `Task`** (without this variant the D6 tasks map would have no writer and every task tool would silently land in the plain-tool ring), and **everything else → a plain tool**. It emits the enum plus the raw tool name and the per-kind extracted facts — file path (from `file_path`/`path`/`pattern`, per `logTailer.ts:455-461`), skill name (`input.skill`), subagent type and model (`input.subagent_type`, `input.model` — the latter present on 16 of 49 observed `Agent` calls, taken because it is free here and the agents box may want it), and the task fields of D9. Every display string (`READ `, `✓`, `✗`, column widths, ordering) is the app's (SD4).
  Note this deliberately **diverges** from the reference: `classifyTool` (`logTailer.ts:444-449`) returns `'skill'` for every unrecognised tool, so a `Bash` call lands in the skills bucket. That fallthrough is a bug, not a pattern to mirror — unknown tools belong in the plain-tool ring.
  Rejected: emitting pre-formatted display strings (SD4, and the crate would need the panel width); classifying app-side (the crate would hand out an untyped bag and then re-derive the same branch twice); mirroring the reference's unknown→skill fallthrough.

### D8: Skills come from invocation events only — never `attributionSkill` — with **no dedupe window** and **no duration**
- **Status**: resolved
- **Rationale**: Three skill-ish signals exist in real logs and only two are events. A `Skill` tool_use carries `input.skill`. A `<command-name>/name</command-name>` wrapper in a user turn is a slash-command invocation — matched in **both** the plain-string content form (observed here) **and** the `message.content[]` array-text form (which `activity.rs:167-170` already handles from other real data, and which the reference's string-only check at `logTailer.ts:295` would miss).
  `attributionSkill` is deliberately **not** an invocation signal: it is ambient attribution stamped on every line of a skill's execution — 41 assistant lines carried `harness-ops:decompose` for one `/decompose` — so keying on it yields 41 entries for one skill.
  **No dedupe window — the two sources cannot collide, by construction.** An earlier revision carried the reference's 10s cross-source window (`logTailer.ts:394-397`). It is unnecessary for a *causal* reason, not merely because a count came out zero: a `<command-name>` wrapper is written when a **human types** `/foo` (it is a user turn), whereas a `Skill` tool_use is written when the **model invokes** a skill. They are different actors, so one invocation produces one signal — which is exactly why `/decompose` shows a wrapper and no tool_use, while `harness-ops:specify` shows a tool_use and no wrapper. The measurement agrees: the collision the window suppresses occurs **zero** times. Worse, the window's endpoints decide everything and 10s is wrong under the reference's own semantics: it compares `tool_result` time to wrapper time, and the `/decompose` run spans **242 seconds** — off by 24×. Shipping an unfalsifiable constant tuned to nothing is worse than shipping neither, so both sources record unconditionally. If a real collision is ever observed, a window can be added then, with endpoints named and a measured value.
  **No duration for skill entries.** A `Skill` tool_use → `tool_result` pair measures **50 ms** — instruction-loading latency, not execution; the skill then runs for minutes across subsequent turns. Emitting it would make `overlay-dashboard-boxes` render `harness-ops:specify 50ms` for a four-minute skill. Skill entries therefore carry a start time only, and D5's duration rule does not apply to `ToolKind::Skill`.
  Rejected: `attributionSkill` as the trigger (41× duplication, measured); string-form-only wrapper detection (misses a form this repo has already hit in production); the reference's 10s cross-source window (refuted above); emitting dispatch latency as if it were run duration (actively misleading).

### D9: Subagents from `Agent` pairs (all depth-0); Tasks from the real task tools, keyed by `taskId`
- **Status**: resolved
- **Rationale**: An `Agent` tool_use opens an active-subagent entry keyed by its `tool_use.id` carrying `subagent_type`, `description`/`prompt`, optional `model` and a start time; the matching `tool_result` marks it completed with an end time. Completed entries stay in the ring — the grace period for hiding them is app-side (D3, SD4).
  **There is no parent linkage, and the spec says so rather than inventing one.** The reference reads `input.parentId` / `input.parent_id` (`logTailer.ts:336-337`) and builds an agent *tree*; that field has **no counterpart in real Claude Code JSONL** — across 49 observed `Agent` calls the input keys are exactly `{subagent_type, run_in_background, description, prompt, model}`. Nested agents write into `<session-uuid>/subagents/agent-*.jsonl`, a directory the session file never references and `newest_jsonl` never reads. Every `Agent` call visible in the session file is therefore **depth-0**, the ring is flat, and `overlay-dashboard-boxes` renders a list, not a tree.
  **Tasks are a distinct, observed source — not parent-less `Agent` calls** (which would make the Tasks store identical to the subagents ring and the two boxes redundant). Measured across 60 project logs: `TaskUpdate` 192, `TaskCreate` 86, `TaskOutput` 8, and **`TodoWrite` 0 — that tool does not exist in this environment** and must not be named. Shapes:
  - `TaskCreate` → `input.{subject, description, activeForm}` (85), plus a batch form `input.tasks` (1).
  - `TaskUpdate` → `input.{taskId, status}` (165), `{taskId, addBlockedBy}` (22), `{taskId, description}` (4), `{taskId, status, description}` (1). `taskId` is a **string** ("1", "10", …) in all 192.
  - `TaskOutput` → `input.{task_id, block, timeout}` — a *read* of a task's output, not a state change; it is classified `Task` but folds nothing.
  **The id↔subject linkage lives in the create's result.** `TaskCreate.input` does not carry the assigned id; its `tool_result` content reads `Task #N created successfully: <subject>` (85 of 86 observed creates match this form; the one miss is the batch `{tasks}` form). The fold pairs them by `tool_use_id` — the subject comes from the pending entry's stored input, `N` from the result — and inserts `N → { subject, status: active, created_ms, updated_ms }` into D6's tasks map.
  **`Task` results never enter the completed-tools ring.** D5's generic rule pushes a closed tool into the ring D7 selects; `ToolKind::Task` has **no** ring, and only `TaskCreate`'s result mutates state. The 196 `TaskUpdate` and 8 `TaskOutput` results measured corpus-wide therefore push nothing — without this rule they would land in the completed-tools ring and evict real tool history at cap 32.
  **Updates apply on the `tool_result`, not the `tool_use`** — and only when it is not `is_error` (D5's outcome rule). A failed update must not move a task's status, and the result is what confirms it applied. The pending entry holds the update payload until then.
  **What an update mutates**, covering all four measured shapes (192 updates): `status` sets the status; `description` **replaces the subject** (it is the only text the update carries, and 5 of 192 use it to reword a task after creation); `addBlockedBy` is **ignored** — dependency wiring the dashboard does not show, and storing it would add a field with no reader (22 of 192 carry it, so an implementer will meet it immediately and must know it is deliberate, not an oversight). An update whose `taskId` is absent from the map (created before the tailed window) is ignored and increments `orphan_task_updates` (D11), exactly as D5 treats an orphan `tool_result`.
  Rejected: `isSidechain` as a subagent signal (uniformly `false` in the parent file — it marks the subagent's *own* transcript lines, in the sibling directory); reconstructing the tree from `parentUuid` (that chains the conversation DAG, not agent parentage); the reference's parent-less-Agent→Task rule (collapses two boxes into one); `TodoWrite` (does not exist here — it came from general knowledge, not this environment); a newest-first ring for tasks (D6 — cannot represent an in-place status change).

### D9a: The task-result prose parse is the one format-fragile step
- **Status**: provisional
- **Rationale**: Every input shape in D9 is measured, but the `taskId` is recovered by matching `Task #(\d+) created successfully:` in the create's `tool_result` **content string** — a human-readable message, not a structured field, and the only linkage available. If that wording changes, new tasks stop being keyed and the map stays empty; existing entries and every other fold are unaffected. The parse must therefore fail soft (no match → increment `unparsed_task_creates` (D11), insert nothing) and must be pinned by a fixture so a wording change breaks a test rather than the dashboard. The batch `TaskCreate {tasks}` form (1 of 86) is likewise unverified against its result shape. Carried to Known Gaps; `overlay-dashboard-boxes` must render an empty Tasks box gracefully, which its accept-seed already requires.

### D10: Unknown and malformed lines are skipped, never fatal — the existing contract is preserved
- **Status**: resolved
- **Rationale**: `apply` takes a line and returns nothing fallible; a line that is not valid JSON increments a `parse_errors` counter and returns, exactly as `parse_line` already behaves (`crates/ai-monitor/src/lib.rs:309-319`). Every field access is a `serde_json::Value` pointer lookup with a typed fallback, so an unknown `type` (8 distinct values in one sampled session, several undocumented) folds whatever it does carry and ignores the rest. Rejected: returning `Result` (every caller would discard it, and a tail loop has nothing useful to do with a bad line but skip it); serde derive structs (a schema mismatch would reject an entire line, and this JSONL is heterogeneous and undocumented enough that that is a certainty, not a risk).

### D11: `snapshot()` returns an owned `TelemetrySnapshot`; `SessionTelemetry` itself is not sent
- **Status**: resolved
- **Rationale**: `SessionTelemetry::snapshot(&self) -> TelemetrySnapshot` yields an owned, `Clone`, borrow-free value — the type `telemetry-watcher` sends over the `mpsc` channel (SD5) and `overlay-dashboard-frame`/`-boxes` format. Because it is the versioned contract two siblings bind to, its contents are enumerated here rather than left as "the counters":
  - **Identity**: `session_id`, `model` (first seen).
  - **Tokens**: `total_input`, `total_output`, `total_cache_read`, `total_cache_creation` (cumulative cost, D2) and `context_used` (latest-value gauge, D2).
  - **Time**: `first_event_ms`, `last_event_ms` (D3 — the app derives uptime/idle).
  - **Activity**: the completed-tools, skills, file-activity and subagents rings, plus the tasks map (D6/D9).
  - **Diagnostics**: `parse_errors` (D10), `usage_conflicts` (D1), `duplicate_tool_ids` (D5), and **three separate orphan counters** — `orphan_tool_results` (D5), `orphan_task_updates` (a `TaskUpdate` for an unknown `taskId`, D9) and `unparsed_task_creates` (a create whose result prose did not match, D9a). They are kept apart deliberately: one shared counter would conflate a mis-sized tail window (expected, benign) with a changed result format (a real regression), destroying the diagnostic value that is the whole reason they exist. These make the failure modes the decisions accept *visible* rather than silent; the app may show them or not (SD4).
  It deliberately **omits** the pending-tool map and the D1 dedupe set — internal bookkeeping the UI never reads. Keeping the snapshot type separate is also what lets the app-side tests build one by hand without driving a fold. Rejected: making `SessionTelemetry` `Clone` and sending it (ships internal bookkeeping across the channel on every append); sending `&SessionTelemetry` (cannot cross a thread boundary through a channel).

## Constraints
- `crates/ai-monitor` stays UI/GPU-free — no `wgpu`, `winit`, `renderer` or `app` import may enter it (SD2, `.claude/rules/dependency-direction.md`)
- No new crate dependencies: `std` + the existing `serde_json` only; `crates/ai-monitor/Cargo.toml` gains no runtime dependency (SD6)
- No file I/O in this feature — `apply` folds strings; the tail loop and the channel belong to `telemetry-watcher`
- Never panic on any input, well-formed or not (existing contract, `crates/ai-monitor/src/lib.rs:309-319`)
- Memory is bounded: every map and buffer that accumulates has a fixed cap and an eviction rule (D6)
- No presentation policy in the crate: no percentages, no formatted strings, no thresholds, no truncation (SD4)
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` stays clean (`CLAUDE.md`)
- Existing public surfaces (`AiEvent`, `parse_line`, `backfill`, `encode_cwd`, `resolve_log*`, `activity::*`) keep working — `crates/app/src/tabmeta` depends on them

## Requirements

### R0: A headless, bounded telemetry fold with an owned snapshot (Confirmed Goal, accept-seed)

#### R0.1: A fixture transcript folds to the expected telemetry
- **Given**: A fixture JSONL transcript checked into `crates/ai-monitor/tests/fixtures/` containing assistant usage lines, nested `tool_use`/`tool_result` pairs, a skill invocation, file tools, task tools and an `Agent` call
- **When**: Every line is passed to `SessionTelemetry::apply` in order and `snapshot()` is taken
- **Then**: Token totals, `first_event_ms`/`last_event_ms`, the tools/skills/file-activity/subagents rings and the tasks map all match asserted values, with no clock read anywhere in the test

#### R0.2: The module is additive and the crate stays headless
- **Given**: The workspace before this feature
- **When**: `cargo build -p ai-monitor` and `cargo tree -p ai-monitor` are run after the change
- **Then**: `telemetry` is a new module reachable from `lib.rs`, no dependency was added to `Cargo.toml`, and the tree contains no `wgpu`, `winit`, `renderer` or `app` entry

### R1: Token accounting — deduplicated totals and a live context gauge (D1, D2)

#### R1.1: Replicated usage lines are folded once
- **Given**: Four JSONL lines sharing one `message.id` and `requestId`, each carrying identical `usage` with `output_tokens: 441`
- **When**: All four are applied
- **Then**: `total_output` is 441, not 1,764

#### R1.2: A retry under a new requestId is folded again
- **Given**: Two lines with the same `message.id` but different `requestId`, each with `output_tokens: 100`
- **When**: Both are applied
- **Then**: `total_output` is 200 — the composite key admits a genuine retry

#### R1.3: A missing requestId still collapses replicas
- **Given**: Two lines sharing a `message.id`, neither carrying `requestId`, each with `output_tokens: 50`
- **When**: Both are applied
- **Then**: `total_output` is 50 — the key degrades to `message.id`, not to per-line counting

#### R1.4: A differing replica is refused and counted
- **Given**: Two lines sharing `(message.id, requestId)` whose `usage` values differ
- **When**: Both are applied
- **Then**: Only the first is folded and `usage_conflicts` is 1

#### R1.5: The context gauge is the latest usage, not a sum
- **Given**: Two distinct messages, the first with `input=2, cache_read=100, cache_creation=10`, the second with `input=3, cache_read=200, cache_creation=20`
- **When**: Both are applied
- **Then**: `context_used` is 223 (the second message's three fields), while `total_cache_read` is 300

#### R1.6: All four cumulative counters accrue independently
- **Given**: Two distinct messages with known `input_tokens`, `output_tokens`, `cache_read_input_tokens` and `cache_creation_input_tokens`
- **When**: Both are applied
- **Then**: `total_input`, `total_output`, `total_cache_read` and `total_cache_creation` each equal the sum of their own field only

### R2: Event time — parsed from the log, never from a clock (D3, D4)

#### R2.1: First and last event times come from the log
- **Given**: Three timestamped lines at `…T07:17:00.461Z`, `…T07:19:00.000Z` and `…T07:22:00.186Z`
- **When**: All three are applied
- **Then**: `first_event_ms` is the epoch-millis of the first and `last_event_ms` of the third

#### R2.2: A line with no timestamp does not move the clock
- **Given**: A telemetry state whose `last_event_ms` is set, followed by a `permission-mode` line carrying no `timestamp`
- **When**: That line is applied
- **Then**: `last_event_ms` is unchanged, and any other content on the line is still folded

#### R2.3: An unparseable timestamp is ignored, not defaulted
- **Given**: A line whose `timestamp` is `"not-a-date"`
- **When**: It is applied
- **Then**: `first_event_ms`/`last_event_ms` are unchanged and no panic occurs

#### R2.4: Uptime and idle are absent from the snapshot
- **Given**: The `TelemetrySnapshot` type
- **When**: Its fields are enumerated
- **Then**: There is no `uptime_ms` or `idle_ms` field — the app derives both from `first_event_ms`/`last_event_ms` (SD4)

### R3: Tool correlation and classification (D5, D7)

#### R3.1: A nested tool_use/tool_result pair completes with a duration
- **Given**: An assistant line whose `message.content[]` holds a `tool_use` with `id: "toolu_1"`, `name: "Bash"` at `T+0s`, then a user line whose content holds a `tool_result` with `tool_use_id: "toolu_1"` at `T+2s`
- **When**: Both are applied
- **Then**: The completed-tools ring holds one entry named `Bash` with `duration_ms: Some(2000)` and success status

#### R3.2: A top-level tool_use/tool_result pair is also correlated
- **Given**: A line with top-level `type: "tool_use"` and a later line with top-level `type: "tool_result"` sharing an id
- **When**: Both are applied
- **Then**: The pair completes exactly as the nested form does

#### R3.3: Only an explicit is_error true is a failure
- **Given**: Three `tool_result`s for three pending tools, with `is_error` absent, `false` and `true` respectively
- **When**: All are applied
- **Then**: The first two record success and only the third records failure

#### R3.4: An orphan tool_result is ignored and counted
- **Given**: A `tool_result` whose `tool_use_id` has no pending entry (the `tool_use` was above the tailed window)
- **When**: It is applied
- **Then**: No ring entry is created and `orphan_tool_results` is 1

#### R3.5: A missing or negative duration is None, never zero
- **Given**: A `tool_use`/`tool_result` pair where the result carries no timestamp, and a second pair where the result's timestamp precedes the `tool_use`'s
- **When**: Both are applied
- **Then**: Both completed entries carry `duration_ms: None`

#### R3.6: A repeated tool_use id replaces the pending entry
- **Given**: Two `tool_use` blocks sharing `id: "toolu_1"` with different start times, then one matching `tool_result`
- **When**: All are applied
- **Then**: The completion is measured from the second `tool_use` and `duplicate_tool_ids` is 1

#### R3.7: Tools classify into the five kinds
- **Given**: `tool_use`s named `Agent`, `Skill`, `Read`, `TaskCreate` and `Bash`
- **When**: Each is classified
- **Then**: They yield `Agent`, `Skill`, `File`(read), `Task` and a plain tool respectively — an unrecognised name never classifies as `Skill`

#### R3.8: File tools extract their path and operation
- **Given**: A `Read` with `input.file_path`, a `Write` with `input.file_path` and a `Grep` with `input.pattern`
- **When**: Each completes
- **Then**: The file-activity ring records the extracted string and the operation read / write / grep

### R4: Every accumulating structure is bounded (D6)

#### R4.1: Display rings evict oldest at their caps
- **Given**: 40 completed plain tools, 20 skills, 40 file activities and 40 subagents applied in order
- **When**: The snapshot is taken
- **Then**: The rings hold 32, 16, 32 and 32 entries respectively, newest-first, with the oldest dropped

#### R4.2: The pending-tool map is capped
- **Given**: 100 `tool_use` blocks with distinct ids and no matching results
- **When**: All are applied
- **Then**: At most 64 pending entries are retained, evicted oldest-first, and memory does not grow with further input

#### R4.3: The dedupe set is capped without corrupting totals in normal use
- **Given**: A transcript of 200 distinct messages, each split across 4 replica lines
- **When**: All are applied
- **Then**: The dedupe set never exceeds 64 entries and `total_output` equals the sum over the 200 distinct messages — replica runs are contiguous, so eviction never splits one

#### R4.4: The tasks map is capped and evicts the least recently updated
- **Given**: 70 created tasks where task #1 has the most recent `updated_ms`
- **When**: The cap is exceeded
- **Then**: The map holds 64 entries and task #1 is retained while the least-recently-updated entries are dropped

### R5: Skills come from invocation events only (D8)

#### R5.1: A Skill tool_use records the skill by name
- **Given**: A `tool_use` named `Skill` with `input.skill: "harness-ops:specify"`
- **When**: It is applied
- **Then**: The skills ring holds one entry named `harness-ops:specify`

#### R5.2: A command-name wrapper is detected in both content forms
- **Given**: One user line whose `message.content` is the plain string `"<command-name>/harness-ops:decompose</command-name>"` and another whose `message.content[]` holds that text in a `text` block
- **When**: Both are applied
- **Then**: Two skill entries are recorded — neither content form is missed

#### R5.3: attributionSkill never creates an entry
- **Given**: 41 assistant lines each carrying `attributionSkill: "harness-ops:decompose"` and no `Skill` tool_use or wrapper
- **When**: All are applied
- **Then**: The skills ring is empty

#### R5.4: Skill entries carry no duration
- **Given**: A `Skill` tool_use at `T+0.186s` and its `tool_result` at `T+0.236s`
- **When**: Both are applied
- **Then**: The skill entry records a start time and no duration — the 50ms dispatch latency is never presented as a run length

### R6: Subagents from Agent pairs, flat (D9)

#### R6.1: An Agent call opens an active subagent
- **Given**: A `tool_use` named `Agent` with `input.subagent_type: "general-purpose"`, `input.description` and `input.model`
- **When**: It is applied
- **Then**: The subagents ring holds one entry, not completed, carrying the type, description and model

#### R6.2: The matching result completes it
- **Given**: An active subagent from R6.1 and its `tool_result` two minutes later
- **When**: The result is applied
- **Then**: The entry is marked completed with an end time, and it remains in the ring (hiding it is app policy)

#### R6.3: The subagent list is flat
- **Given**: Three `Agent` calls in one transcript
- **When**: The snapshot is taken
- **Then**: All three are depth-0 siblings and the entry type carries no parent field

### R7: Tasks are a keyed map fed by the task tools (D9, D9a)

#### R7.1: A create pairs its input subject with the id from its result
- **Given**: A `TaskCreate` `tool_use` with `input.subject: "T1: Split title model"` and `id: "toolu_9"`, then a `tool_result` for `toolu_9` whose content is `"Task #1 created successfully: T1: Split title model"`
- **When**: Both are applied
- **Then**: The tasks map holds key `"1"` with that subject and an active status

#### R7.2: An update mutates the entry in place on its result
- **Given**: Task `"1"` exists and a `TaskUpdate` with `input: {taskId: "1", status: "in_progress"}` completes without error
- **When**: The `tool_result` is applied
- **Then**: Task `"1"` has status `in_progress`, `updated_ms` advanced, and the map still holds exactly one entry

#### R7.3: A failed update does not mutate
- **Given**: Task `"1"` with status active and a `TaskUpdate` whose `tool_result` carries `is_error: true`
- **When**: The result is applied
- **Then**: Task `"1"` retains its previous status

#### R7.4: description rewords the subject; addBlockedBy is ignored
- **Given**: Task `"1"` exists, then `TaskUpdate {taskId: "1", description: "T1: revised"}` and `TaskUpdate {taskId: "1", addBlockedBy: "2"}`
- **When**: Both complete
- **Then**: The subject becomes `"T1: revised"` and no blocked-by state is stored

#### R7.5: An update for an unknown id is ignored and counted
- **Given**: An empty tasks map and `TaskUpdate {taskId: "7", status: "completed"}`
- **When**: It completes
- **Then**: The map stays empty and `orphan_task_updates` is 1

#### R7.6: An unparseable create result inserts nothing
- **Given**: A `TaskCreate` whose `tool_result` content does not match `Task #N created successfully:` (e.g. the batch `{tasks}` form)
- **When**: It is applied
- **Then**: No task is inserted and `unparsed_task_creates` is 1

#### R7.7: Task results never enter the completed-tools ring
- **Given**: One `TaskCreate`, one `TaskUpdate` and one `TaskOutput`, each with a matching result
- **When**: All are applied
- **Then**: The completed-tools ring is empty — only the tasks map changed

### R8: Malformed and unknown input is survivable (D10)

#### R8.1: Invalid JSON is counted, never fatal
- **Given**: The lines `"not json at all"`, `""` and `"{ truncated"`
- **When**: Each is applied
- **Then**: No panic occurs and `parse_errors` counts the two non-empty invalid lines

#### R8.2: An unknown line type folds what it carries
- **Given**: A line of type `file-history-delta` carrying a valid `timestamp` and nothing else recognised
- **When**: It is applied
- **Then**: `last_event_ms` advances and no other state changes

#### R8.3: Missing and wrongly-typed fields degrade gracefully
- **Given**: A line whose `message.usage.input_tokens` is the string `"12"` and whose `message.content` is a number
- **When**: It is applied
- **Then**: The unreadable fields are skipped, readable ones are folded, and no panic occurs

### R9: The snapshot is the sibling-facing contract (D11, SD2, SD4, SD5)

#### R9.1: The snapshot is owned, cloneable and sendable
- **Given**: A populated `SessionTelemetry`
- **When**: `snapshot()` is called and the result is moved to another thread through an `mpsc` channel
- **Then**: It compiles and the received value equals the sent one — the type is `Clone + Send` with no borrows

#### R9.2: Internal bookkeeping is not exposed
- **Given**: The `TelemetrySnapshot` type
- **When**: Its fields are enumerated
- **Then**: The pending-tool map and the dedupe set are absent, while the counters, rings, tasks map and all six diagnostics are present

#### R9.3: No presentation policy is exposed
- **Given**: The public API of the `telemetry` module
- **When**: It is inspected
- **Then**: There is no percentage, no formatted string, no column width and no per-model table — every value is a raw number, enum or unformatted string (SD4)

### R10: Verification (SD6, SD7)

#### R10.1: Fixtures are checked in and drive the assertions
- **Given**: The test suite
- **When**: `cargo test -p ai-monitor` runs on a machine with no `~/.claude` directory
- **Then**: Every telemetry test passes, reading only files under `crates/ai-monitor/tests/fixtures/`

#### R10.2: The result-prose parse is pinned by a fixture
- **Given**: The R7.1 fixture containing the literal `Task #1 created successfully:` wording
- **When**: That wording changes upstream
- **Then**: The fixture test fails, surfacing the format break rather than silently emptying the tasks map

#### R10.3: Lint and existing behaviour hold
- **Given**: The workspace after this feature
- **When**: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` run
- **Then**: Both pass, including the pre-existing `ai-monitor` and `tabmeta` tests that depend on `parse_line`, `backfill` and `activity::*`

## Known Gaps
- **Token totals EXCLUDE subagent work — decided, not accidental.** Verified: subagent transcripts live in `<session-uuid>/subagents/agent-*.jsonl` with `isSidechain: true` and their own `message.usage`, and `newest_jsonl` (`crates/ai-monitor/src/lib.rs:56-72`) is non-recursive, so they are never read. The dashboard therefore reports **main-thread tokens only**, and a delegation-heavy session **under-reports** its true spend — potentially by a lot (one subagent file in this very project is 224 KB against a 158-line parent). This is accepted for v1: the overlay's subject is the session you are watching, and the subagents ring already shows that delegation happened. Revisit if the under-report becomes misleading; folding the sibling directory is a `telemetry-watcher` concern (a second watch target), not a change to this fold.
- **The `Task #N created successfully:` result-prose parse is format-fragile** (D9a) — it is the only `taskId ↔ subject` linkage and it reads a human-readable string. Pin it with a fixture; a wording change must break a test, not the dashboard. The batch `TaskCreate {tasks}` form (1 of 86 observed) has an unverified result shape.
- **L2 provisional — Data Model:** D9a (the task-result prose parse) is the one checkpoint not resolved to a structured, observed field. Everything else in D9 is measured across 60 logs.
- **Thinking/thought tokens are not modelled.** The TS state carries `thoughtTokens`, but no such field appears in Claude Code's observed `usage` keys. Tokens for `thinking` blocks are presumably already inside `output_tokens`; unconfirmed.
- **`cache_creation` (object) vs `cache_creation_input_tokens` (number).** Both keys appear in `usage`; only the scalar is folded. The object's breakdown (presumably per-TTL) is ignored.
- **Retry/resume behaviour is unfalsified, not verified.** D1's composite key exists to let a genuine retry through, but **zero** retries occur across 5,715 usage-bearing lines in 60 logs — no `message.id` was seen under two `requestId`s. That is much stronger than "not in the sampled session", but it is still absence of evidence: the key is defensive. The `usage_conflicts` counter (D1) is the instrument that would reveal one.
- Gemini CLI logs are out of scope; `geminiLogTailer.ts` has no counterpart here.

## Tasks

> **On parallelism.** The approved declared surface names `crates/ai-monitor/src/telemetry.rs` as a **bare path**, which under the T1 glob semantics matches that one file only — a `telemetry/` directory of submodules would fall outside the surface this set was approved with. The fold therefore stays one file, and because every task below edits it, the file-overlap rule serialises them. This DAG is near-linear by construction, not by under-analysis. T4 and T5 are the one genuine fork — both depend only on T3 and add independent `match` arms, so they may run concurrently if a worker is willing to reconcile two edits to the same file.

### T1: Module skeleton — types, dispatch, JSON and time helpers, diagnostics [infra]
- **Fulfills**: R0.2, R2.2, R2.3, R8, R9.1, R9.2, R9.3
- **Depends on**: (none)
- Declare `pub mod telemetry;` in `lib.rs`. Define `SessionTelemetry`, `TelemetrySnapshot` (the full D11 field list including all six diagnostics), `ToolKind`, and the completed-tool / skill / file-activity / subagent / task entry types. Implement `apply` as a dispatch shell that parses the line to `Value`, counts `parse_errors` on failure, and returns; the pointer-lookup helpers with typed fallbacks; and the RFC 3339 `…Z` → epoch-millis parse using `std` only. No dependency may be added to `Cargo.toml`.

### T2: Token accounting — composite-key dedupe, cumulative totals, context gauge [vertical]
- **Fulfills**: R1, R2.1
- **Depends on**: T1
- Fold `message.usage` / top-level `usage` once per `(message.id, requestId)` with the documented fallbacks; accrue the four cumulative counters; set `context_used` from the latest folded usage; count `usage_conflicts` on a differing replica; advance `first_event_ms` / `last_event_ms` from parsed timestamps only.

### T3: Tool correlation and classification [vertical]
- **Fulfills**: R3
- **Depends on**: T2
- Collect `tool_use` / `tool_result` from both the nested `message.content[]` form and the top-level event form; maintain the pending map keyed by `tool_use.id`; classify into the five `ToolKind` variants (unknown → plain tool, never skill); extract file path and operation; apply the outcome rule (only explicit `is_error: true` fails), the duration rules (`None` when missing or negative, never zero), and the orphan / duplicate-id counters.

### T4: Skills [vertical]
- **Fulfills**: R5
- **Depends on**: T3
- Record from the `Skill` tool_use (`input.skill`) and from `<command-name>` wrappers in **both** the plain-string and array-text content forms. Never key on `attributionSkill`. Emit no duration for skill entries and no dedupe window.

### T5: Subagents [vertical]
- **Fulfills**: R6
- **Depends on**: T3
- Open an entry on an `Agent` tool_use carrying `subagent_type`, `description`/`prompt` and `model`; complete it on the matching result. Flat list, no parent field.

### T6: Tasks — keyed map fed by the task tools [vertical]
- **Fulfills**: R7
- **Depends on**: T5
- Classify `TaskCreate|TaskUpdate|TaskOutput` as `ToolKind::Task`; pair a create's stored input subject with the `Task #N created successfully:` id parsed from its result; mutate on a non-error update result (`status` sets status, `description` replaces the subject, `addBlockedBy` ignored); count `orphan_task_updates` and `unparsed_task_creates`; ensure no `Task` completion reaches the completed-tools ring.

### T7: Bounding pass — caps and eviction across every store [infra]
- **Fulfills**: R4
- **Depends on**: T4, T6
- Apply the D6 caps: rings at 32 / 16 / 32 / 32 newest-first; pending map 64 oldest-first; dedupe set 64 oldest-first; tasks map 64 evicted by **oldest `updated_ms`**. Done as one pass after every store exists so a single eviction policy is applied consistently rather than five ad-hoc ones.

### T8: Fixtures, tests and lint [infra]
- **Fulfills**: R0.1, R10
- **Depends on**: T7
- Check fixture JSONL into `crates/ai-monitor/tests/fixtures/` — including the literal `Task #1 created successfully:` wording that R10.2 pins — and assert every sub-requirement above with plain assertions, reading no file outside the fixtures dir and no clock. Confirm `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass, including the pre-existing `parse_line` / `backfill` / `activity` / `tabmeta` tests.

## External Dependencies

### Pre-work
- (none) — every input shape is already measured and the fixtures are authored from logs already on this machine.

### Post-work
- Re-sample a session containing `Agent` calls and one using the task tools, to close the two Known Gaps (subagent token exclusion; the batch `TaskCreate {tasks}` result shape). Not a blocker for this feature.
