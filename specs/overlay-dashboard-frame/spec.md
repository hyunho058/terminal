# Spec: overlay-dashboard-frame

## Meta
- **Created**: 2026-07-28
- **Type**: dev
- **Status**: ready
- **Set**: ai-status-dashboard (`specs/ai-status-dashboard/partition-manifest.md`)
- **Mode**: batch (`pre-approved-batch: yes`) — L0/L2/L3/L4 human prompts skipped under decompose's single partition gate; the L2-reviewer, coverage, GWT and DAG gates all ran
- **Approved by**: decompose partition gate (`specs/ai-status-dashboard/partition-manifest.md`)
- **Approved at**: 2026-07-28

## Declared Surface
- feature-id: overlay-dashboard-frame
- depends_on: [telemetry-watcher]
- declared-surface:
  - crates/app/src/dashboard.rs
  - crates/app/src/overlay.rs
  - crates/app/src/main.rs
  - crates/app/Cargo.toml

## Goal
Give the overlay a real header: a new `crates/app/src/dashboard.rs` owning the layout policy — box-drawing composition helpers plus the header section (session/model, input/output/cache tokens, context %, uptime/idle) — with the per-model context-window table living app-side, per the D21 policy/mechanism split. Compose into `Vec<String>` using U+2500–259F box drawing, which renders natively since `d003ab2`.

## Non-goals
- The activity sections — Tasks, Active Agents, Skills, Recent Tools, File Activity are `overlay-dashboard-boxes`
- Any colour or styled spans (SD3 — a separate slice)
- Changing `OverlayModel`, the renderer, `seam.rs` or `terminal_app.rs` (all outside this surface)
- Changing the telemetry fold or the watcher

## Confirmed Goal
`crates/app/src/dashboard.rs` exposes a **pure** function from a `TelemetrySnapshot` plus a caller-supplied `now_ms` to `Vec<String>`, composing a box-drawn header with session/model, cumulative token spend, context occupancy as a percentage of an app-side per-model window table, and uptime/idle derived from the snapshot's first/last event times. `overlay.rs` is reduced to wiring: it calls that function and pushes the result into `OverlayHandle`. The composition is `insta`-snapshot tested against hand-built snapshots, with no window, no GPU and no clock.

## Research
- `OverlayModel` is `{ visible: bool, lines: Vec<String> }` (`crates/renderer/src/overlay_layer.rs:23`) — no styling, and **no width field**.
- **There is no width channel to the app.** `AppCtx` (`crates/app/src/seam.rs:46-71`) carries `writer`, `tab_count`, `active`, `active_cwd`, `bracketed_paste`, `selection_text` — no columns, no cell size — and the `Overlay` trait has no resize hook. The panel's column budget is computed *inside the renderer at draw time*: `max_cols = (panel_width - 2*PAD) / cell_width` (`overlay_layer.rs:264`), and each line is then clipped with `truncate_cols` (`:272`). So the app composes blind and the renderer clips.
- The panel is the lower 55% of the window, full width, below the tab-bar inset (`overlay_layer.rs:299-300`), drawn oldest-first from the top with the last `max_lines` shown (`:262-270`).
- Clipping is **column**-aware, not char-aware — Hangul spends 2 columns — via the shared `truncate_cols`/`layout_line` helpers the tab bar also uses (`overlay_layer.rs:256-283`, test at `:463-473`). Box-drawing characters are width-1, so a rule of N glyphs is N columns.
- Box drawing renders as cell-snapped procedural quads since `d003ab2` — no glyph atlas round-trip, so rules and corners are crisp at any size. Precisely: **157 of the 160 codepoints** in `U+2500..=U+259F` are procedural; the three diagonals `╱╲╳` (U+2571–2573) are excluded and fall back to the font (`crates/renderer/src/box_draw.rs:1-5`, pinned by `exactly_157_of_160_codepoints_are_procedural`). SD3's shorthand "U+2500–259F" is inherited verbatim from the manifest; this is the exact statement. Nothing in this feature uses a diagonal.
- `SidebarGeom { sidebar_w, viewport_h, cell, scale, … }` is published back to the app by the sidebar layer after it draws, readable via `renderer::sidebar_row::TabModelHandle::shared()` (`crates/renderer/src/sidebar_row.rs:159-167,371-373`), and `crates/app/src/main.rs:72` already calls it. It carries cell size and viewport **height** — but `sidebar_w`, not viewport width.
- `WindowEvent::Resized` resizes the renderer and redraws but does **not** poll the overlays (`crates/app/src/terminal_app.rs:1298-1307`), and the loop is `ControlFlow::Wait` — so nothing recomposes the overlay except a snapshot arrival or the toggle chord.
- Default window is 900×540 **logical** (`crates/app/src/terminal_app.rs:230`); cell size is `(round(size_pt·scale·0.6), round(size_pt·scale·1.2))` (`crates/renderer/src/lib.rs:1349-1352`) with `size_pt: 14.0` by default (`crates/common/src/config.rs:87`). At 2× scale that gives `cw = 17`, `ch = 34`, so a full-width panel budgets `max_cols = floor((1800 − 20)/17) = 104` columns and `max_lines = floor((594 − 20)/39.1) = 14` lines — the basis for `RULE_COLS` in F1, and for the height budget `overlay-dashboard-boxes` sizes against.
- The TS reference's header carries: session label + model + status badge; `up`/`idle`; `context N%`; parse errors; and `in`/`out`/`$cache` token pills, wrapped to width (`src/ui/components/header.ts:71-133`). Its `MODEL_LIMITS` table stops at `claude-opus-4` and defaults to 200k (`:36-53`).
- The live session's model is **`claude-opus-5`** — absent from that table. (An earlier revision of this spec also claimed model ids carry a `[1m]` context-window suffix. **Refuted**: zero of 28,925 logged `message.model` values contain a bracket — see F2. The claim is recorded here only so it is not reintroduced; the real, unfixed problem it gestured at is in Known Gaps.)
- `telemetry-model` D2 establishes that the four cumulative totals are **cost** counters, not occupancy: cumulative cache-read reached ≈1.9M against a live context of ≈117k on one sampled session (≈16×). It explicitly requires this feature to label them as spend, never as "context used".
- `telemetry-model` D3 keeps uptime/idle out of the snapshot deliberately — the crate exposes `first_event_ms`/`last_event_ms` and the app subtracts its own clock.
- `term-core` already uses `insta` for snapshot tests (`crates/term-core/Cargo.toml:19-20`); `crates/app` has **no** `[dev-dependencies]` section yet.

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

### F1: Sections are **open-right**, never closed boxes — because there is no width channel
- **Status**: resolved
- **Rationale**: This is the load-bearing consequence of SD3. `OverlayModel` has no width field, `AppCtx` carries no columns, and the `Overlay` trait has no resize hook — so the app composes lines **blind** while the renderer clips them to a `max_cols` it computes at draw time (`overlay_layer.rs:264,272`). A closed box (`╔═ Label ═╗` … `╚════╝`, the TS reference's `sectionTitleDouble`) requires knowing the exact column budget: too wide and the closing `╗` is silently clipped off, leaving a ragged unterminated border; too narrow and the box floats in dead space on a wide window. Sections are therefore **left-anchored and open on the right** — `── Session ──────────────` with a trailing rule and no closing glyph — mirroring the reference's *other* helper, `sectionTitle` (`utils.ts:3-7`). Clipping such a line is harmless: a rule simply ends earlier. The rule is emitted to a nominal `RULE_COLS` and content rows carry no right border.
  **`RULE_COLS = 104`** — the nominal rule length, stated as a number because it is the single most layout-determining constant here. Derivation, at the default 900×540 logical window with default `size_pt: 14.0` on a 2× display: `cw = round(14 × 2 × 0.6) = 17`; the panel spans the full viewport width, so `max_cols = floor((1800 − 2×10)/17) = 104` (`overlay_layer.rs:264`, `PAD = 10`). The rule therefore terminates almost exactly at the panel edge on a default Retina window; at 1× the budget is 110, so it runs 6 columns short; on any narrower window it is clipped mid-rule. That is the least conspicuous direction to be wrong in — a rule is self-similar, so a clipped one still reads as a rule, where a clipped *closed* box loses its corner and reads as broken.
  **Why the app cannot simply learn the width — and why that is not the only reason.** Two distinct facts, and an earlier revision conflated them:
  1. *Surface, not architecture.* The claim that a width channel is "the same kind of contract extension as styled spans" is **too strong**. `renderer::sidebar_row::TabModelHandle::shared()` already publishes draw-time geometry back to the app — `SidebarGeom { cell, viewport_h, scale, sidebar_w, … }`, written by the sidebar layer after it draws, and `main.rs:72` already calls it. So an in-repo precedent exists and the missing quantity is one `f32` (viewport *width*; `sidebar_w` is published, viewport width is not). What blocks it here is that `overlay_layer.rs` and `sidebar_row.rs` are **outside this feature's declared surface**, not that the architecture forbids it.
  2. *Freshness, which no channel fixes on its own.* The composer runs only when a snapshot arrives (`Overlay::poll`) or on the toggle chord. `WindowEvent::Resized` (`terminal_app.rs:1298-1307`) resizes the renderer and redraws but **does not poll the overlays**, and under `ControlFlow::Wait` there is no tick. So any width the app learned would be stale from the first resize until the next log append. **This is the strongest argument for open-right**: a width-independent layout is correct at every window size without a recompose, where a width-dependent one is correct only until the user drags the window edge.
  Rejected: closed boxes at a fixed width (clipped mid-border or floating in dead space, and the missing `╗` is a conspicuous tell); guessing width from config font size (wrong after any resize, per (2)); reading viewport width through the sidebar handle (out of surface, and still stale per (2)); extending `OverlayModel` with a `cols` hint now (SD3 defers contract changes and `overlay_layer.rs` is out of surface).

### F2: The per-model context-window table is app-side, longest-prefix, seeded from measured ids
- **Status**: resolved
- **Rationale**: SD4 puts the table here, not in the headless crate, precisely so a new model needs no `ai-monitor` change. Lookup is **longest-prefix, then default** — one rule, no second mechanism.
  **Seeded from what the logs actually contain**, measured over `message.model` on assistant lines across every file under `~/.claude/projects` — 28,925 values: `claude-opus-4-8` (24,693), `claude-opus-5` (4,020), `claude-haiku-4-5-20251001` (123), `claude-fable-5` (61), `<synthetic>` (28). The table therefore carries `claude-opus-`, `claude-sonnet-`, `claude-haiku-`, `claude-fable-` at 200k, leaving `<synthetic>` as the only unmatched value (F2a). The TS reference's table stops at `claude-opus-4` and matches none of these (`header.ts:36-53`), which is why it is re-derived rather than copied.
  **`message.model` is the right field, and a bare `opus` is not a counter-example.** A raw scan for `"model":` also turns up `opus` (73), but those live in **`Agent` tool-call inputs** (`input.model`, alongside `subagent_type` values like `general-purpose`), not in `message.model` — they are per-subagent model overrides, and `TelemetrySnapshot.model` reads the session's model (`telemetry-model` D11). Short aliases therefore never reach this table. (`overlay-dashboard-boxes` *does* display `input.model` in its agents rows, where `opus` is exactly the right string to show.)
  **An earlier revision invented a `[1m]` context-window suffix rule** (`claude-opus-5[1m]` → 1M). That form appears **zero times** in 28,834 logged values — it is the agent-runtime model id, not what Claude Code writes to JSONL, and `TelemetrySnapshot.model` comes from `message.model`. The rule was unreachable over real data, and it did not fix the failure it named: a genuine 1M-context session still logs as bare `claude-opus-5`. It is removed rather than kept as dead defensive code.
  Rejected: the reference's table verbatim (matches no current model); the `[1m]` suffix rule (unreachable — see above); deriving the window from observed peak usage (a fresh session has no peak, and it would ratchet upward, never down).

### F2a: An unmatched model omits context %, rather than guessing — a correctness rule, not a frequency one
- **Status**: resolved
- **Rationale**: This resolves what would otherwise be a contradiction between F2's "200k default" and F7's "omit when the model is unknown". **F7 wins, and F2's default is narrowed to a fallback for *sizing only*, never for display.**
  Against the measured census the unmatched set is exactly **`{<synthetic>}`** — 28 of 28,925 values, 0.1%. Every other observed id matches a seeded prefix, including `claude-fable-5` (→ `claude-fable-`). So this rule is **not** justified by how often it fires; it is justified by what a wrong answer costs. A percentage is read as a budget, and a confident number against a guessed denominator is a false statement about how much room the session has left — the same reasoning `telemetry-model` D4 used to refuse a `Date.now()` timestamp fallback. Rarity is a reason to keep the rule cheap, not a reason to skip it.
  Rejected: defaulting to 200k and showing a percentage anyway (silently wrong for any model whose window differs — and per the Known Gap below, that includes the case this feature most wants to get right); showing `?%` (visual noise that says the same thing as omission); skipping the rule because 0.1% is rare (a dashboard that is confidently wrong 0.1% of the time is worse than one that is silent, because nothing marks which reading was the bad one).

### F3: Context % uses the gauge; token totals are labelled as **spend**
- **Status**: resolved
- **Rationale**: `telemetry-model` D2 draws a hard line: `context_used` is a latest-value occupancy gauge, while the four cumulative counters are cost. The header shows `context N%` from `context_used / window`, and shows `in` / `out` / `cache` as cumulative **spend** with an explicit unit framing (e.g. `billed`), never adjacent to the percentage in a way that reads as occupancy. On the sampled session those two families differ by ≈16× (≈1.9M cumulative cache-read vs ≈117k live context) — mislabelling them would make a healthy session look catastrophically over budget. Rejected: showing a percentage derived from cumulative tokens (nonsense — it exceeds 100% within minutes); omitting the totals (they are the session's real cost, and the goal asks for token counts).

### F4: Uptime and idle are computed here from an injected clock — and labelled "as of last update"
- **Status**: resolved
- **Rationale**: `telemetry-model` D3 deliberately keeps `uptime_ms`/`idle_ms` out of the snapshot so the fold stays pure and fixture-testable. The app therefore computes `uptime = now_ms - first_event_ms` and `idle = now_ms - last_event_ms`, saturating at zero. The clock is **injected as a `now_ms` parameter**, not read inside the composer — that is what keeps `dashboard.rs` pure and snapshot-testable (F6). `overlay.rs` supplies the real time at the call site.
  **These two fields are stale between snapshots, and the header must not pretend otherwise.** The composer runs only on snapshot arrival or the toggle chord; there is no periodic tick under `ControlFlow::Wait`. So `idle` — the one value whose entire purpose is to grow while nothing happens — freezes at whatever it was when the last line was folded, and reads ≈0 throughout exactly the idle period it exists to signal. Since a periodic recompose would need a change in `terminal_app.rs`/`seam.rs` (outside this surface), the honest fix here is presentational: both values are rendered under an explicit *as of last update* framing rather than as live counters. Re-opening the overlay recomposes and gives a true reading on demand.
  Rejected: reading the clock inside the composer (every `insta` snapshot would change every run, and it would still not refresh the panel — nothing triggers a redraw); moving the computation into `ai-monitor` (SD4, and it re-introduces the clock dependency D3 removed); presenting `idle` as a live counter (it is not one, and a frozen ≈0 idle on a long-idle session is the single most misleading thing this header could show).

### F3a: The two worked examples that pin the header's shape
- **Status**: resolved
- **Rationale**: The deliverable is text pinned by `insta`, so without a worked example the first accepted snapshot silently becomes the specification. These two are normative for field order, separators, abbreviation and placeholders; `RULE_COLS = 104` rules are shown truncated here for readability.

  **Populated** — `claude-opus-5`, 117,066 of a 200k window, 45 min uptime, 2 min since the last event:
  ```
  ── ai-monitor ──────────────────────────────────────────────
   session  5e59b629 · claude-opus-5 · context 59%
   spend    in 45 · out 19.3K · cache 1.9M (billed)
   activity up 45m12s · idle 2m03s (as of last update)
  ```

  **Empty** — overlay opened, nothing folded yet (or no log resolved):
  ```
  ── ai-monitor ──────────────────────────────────────────────
   session  — · —
   spend    in — · out — · cache — (billed)
   activity — (as of last update)
  ```

  Rules pinned by these examples, all width-independent so they hold at any panel size:
  - **Separator is exactly `" · "`** — one space, U+00B7, one space — on every row. No double spacing, and **no padding to a column**: aligning the qualifiers would be the fixed-width assumption F1 rejects, and it would break the moment a value changed length.
  - **Qualifiers follow their last value after a single space.** `(billed)` and `(as of last update)` sit at whatever column the content ends at. F1's open-right rule means a narrow panel clips the qualifier before any value — the intended degradation, since the numbers matter and the caveat is a reminder.
  - **Row labels are padded to 8 columns** (`session `, `spend   `, `activity`) — a fixed *left* gutter, which needs no width knowledge, unlike a right-hand column.
  - Numbers abbreviate at 1e3/1e6 to one decimal (`19.3K`, `1.9M`) and print bare below 1,000 (`45`). The placeholder is U+2014 `—`. `context N%` rounds to the nearest integer and is **omitted entirely** — label and all — when the model is unmatched (F2a) or `context_used` is zero (F7). Durations use `45m12s` / `2m03s` / `1h05m`. Session id is the first 8 characters (`header.ts:72-76`).
  Rejected: a single-line header (four values plus qualifiers do not fit `RULE_COLS` at any realistic width); right-aligned or column-padded qualifiers (impossible width-blind, per F1 — and the earlier revision of this example did it inconsistently across the two cases, which is exactly the ambiguity this block exists to remove); thousands separators (locale-dependent and wider than the abbreviation).

### F3b: `cache` is `cache_read + cache_creation`, summed because both are billed
- **Status**: resolved
- **Rationale**: `telemetry-model` D11 exposes four cumulative counters but F3a's spend row shows three fields, so `cache` must be defined or the composer cannot be written. It is **`total_cache_read + total_cache_creation`**. The row's subject is *spend* (F3), and both counters are billed tokens, so summing them answers the question the row asks; splitting them would add a fourth field for a distinction that matters when tuning caching, not when watching a session. The two are separately available in the snapshot if a later slice wants a breakdown. Rejected: showing `total_cache_read` alone (silently drops cache-creation spend, which is the *more* expensive of the two per token); four separate fields (the row stops fitting `RULE_COLS`, and the extra precision serves no question this dashboard asks); merging cache into `in` (D2 keeps the gauge and the cost counters distinct — burying cache inside input tokens would undo that).

### F5: `dashboard.rs` is a pure `Vec<String>` composer; `overlay.rs` becomes wiring
- **Status**: resolved
- **Rationale**: The public entry point is a pure function of `(&TelemetrySnapshot, now_ms) -> Vec<String>` — no I/O, no globals, no clock, no renderer types. That is what makes the layout `insta`-testable with zero GPU (SD7) and what lets `overlay-dashboard-boxes` extend it by adding sections to the same composer rather than touching `overlay.rs` again. `overlay.rs` keeps only lifecycle and wiring: resolve log, backfill, spawn watcher, drain snapshots, call the composer, push into `OverlayHandle`. Rejected: composing inside `overlay.rs` (drags the `Overlay` trait, `EventLoopProxy` and watcher lifecycle into every layout test); returning a single `String` with embedded newlines (`OverlayModel.lines` is already a `Vec`, and the renderer draws one line per element).

### F6: Layout is verified with `insta` snapshots over hand-built telemetry
- **Status**: resolved
- **Rationale**: The deliverable is *text*, so a snapshot test is exactly the right instrument: a layout change shows up as a reviewable diff instead of a broken assertion, and the project already has the `/insta-review` workflow. `crates/app/Cargo.toml` gains `insta` under `[dev-dependencies]` only.
  **This does not narrow SD6.** SD6's title reads set-wide, but its rationale is entirely about `crates/ai-monitor`'s runtime dependencies, and the stronger fact settles it either way: `insta = "1.48.0"` is **already in the workspace** (`crates/term-core/Cargo.toml:19-20`), so pinning the same version adds **no new crate to the dependency tree at all** — it is a dev-dependency edge to a crate already vendored. Nothing about SD6's intent is touched. Snapshots are taken over hand-constructed `TelemetrySnapshot` values at a fixed `now_ms`, never a live log.
  Rejected: `assert_eq!` on whole strings (unreadable diffs, and every layout tweak becomes a manual rewrite); testing through the renderer (needs a GPU and asserts pixels for a text change); a different snapshot crate (would genuinely add a new dependency, and diverge from `/insta-review`).

### F7: Every field degrades to a visible placeholder, never a blank or a lie
- **Status**: resolved
- **Rationale**: A snapshot legitimately arrives empty — the overlay opens before any line is folded, or no log resolved at all (`overlay.rs` holds `Option<TelemetrySnapshot>` per `telemetry-watcher` W5). The header must still render its frame with `—` in place of each missing value, and context % is **omitted entirely** (not shown as `0%`) when either `context_used` is zero or the model is unknown — a confident `0%` is a false statement about a session whose occupancy is simply unknown. Uptime/idle are omitted when `first_event_ms`/`last_event_ms` are `None` rather than showing the epoch. Rejected: rendering nothing until data arrives (the panel would flash empty on every open); showing `0` for unknowns (indistinguishable from a real zero).

### F8: `main.rs` gains exactly one line
- **Status**: resolved
- **Rationale**: `crates/app` is a binary whose modules are declared in `main.rs` (`:10-21`); the only change there is `mod dashboard;`, inserted in the existing alphabetical run. Keeping that edit to one line is what makes `main.rs` safe to share as a declared surface — `overlay-dashboard-boxes` does not touch it at all, since it only extends `dashboard.rs`. Rejected: a `dashboard/` directory module (the declared surface names `crates/app/src/dashboard.rs`, a bare path, which under the T1 glob semantics matches that one file only — a directory would fall outside the approved surface).

## Constraints
- `crates/renderer/**`, `crates/app/src/seam.rs` and `crates/app/src/terminal_app.rs` are outside the declared surface and must not be edited
- `OverlayModel` stays `{ visible, lines }` — no styling, no width field (SD3)
- `dashboard.rs` must not import `winit`, `wgpu` or any renderer type; its public composer takes only a `&TelemetrySnapshot` and a `now_ms`
- The composer is pure: no clock read, no filesystem, no globals
- `crates/app/Cargo.toml` may gain `insta` under `[dev-dependencies]` only, pinned to the workspace's existing `1.48.0` — no runtime dependency, no new crate in the tree
- The `now_ms` conversion in `overlay.rs` must not `unwrap`: `SystemTime::now().duration_since(UNIX_EPOCH)` is fallible and `as_millis()` is `u128`, so the call site uses a saturating conversion with a documented fallback (a clock before the epoch yields `0`, which F7 then renders as the `—` placeholder)
- `crates/app/src/dashboard.rs` is a single file (bare-path glob), not a directory module
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` stays clean

## Requirements

### R0: A box-drawn header composed from telemetry (Confirmed Goal, accept-seed)

#### R0.1: The populated header matches its pinned form
- **Given**: A hand-built snapshot with `session_id`, `model: "claude-opus-5"`, `context_used: 117066`, the four token totals, and first/last event times
- **When**: The composer runs at a fixed `now_ms`
- **Then**: The output equals F3a's populated example — rule, session row, spend row, activity row — pinned by an `insta` snapshot

#### R0.2: `overlay.rs` holds no layout
- **Given**: The diff
- **When**: `overlay.rs` is inspected
- **Then**: It calls the composer and pushes the result into `OverlayHandle`; no box-drawing character, column constant or model table appears in it

### R1: Sections are open-right at a fixed nominal width (F1)

#### R1.1: The rule is `RULE_COLS` columns and has no closing glyph
- **Given**: Any snapshot
- **When**: The header is composed
- **Then**: The rule line is exactly 104 columns, starts `── ai-monitor ─`, and ends in `─` — never `╗`, `┐` or any corner

#### R1.2: Clipping a rule degrades harmlessly
- **Given**: A composed rule of 104 columns and a panel budget of 81
- **When**: The renderer applies `truncate_cols`
- **Then**: The line remains a valid rule with no orphaned corner glyph

### R2: The context-window table and the percentage (F2, F2a, F3)

#### R2.1: A seeded prefix yields a percentage
- **Given**: `model: "claude-opus-5"` and `context_used: 117066`
- **When**: The header is composed
- **Then**: It shows `context 59%` — `117066/200000` rounded to the nearest integer

#### R2.2: Longest prefix wins
- **Given**: `model: "claude-haiku-4-5-20251001"`
- **When**: The window is resolved
- **Then**: It matches `claude-haiku-` and yields 200,000

#### R2.3: An unmatched model omits the percentage entirely
- **Given**: `model: "<synthetic>"` and a non-zero `context_used`
- **When**: The header is composed
- **Then**: The session row shows the model name and **no** `context` label or value — not `0%`, not `—%`

#### R2.4: No suffix parsing exists
- **Given**: The implementation
- **When**: The model-lookup code is inspected
- **Then**: There is no bracket-stripping or suffix rule — lookup is longest-prefix then default, one path

### R3: Spend is labelled as cost, never occupancy (F3, F3b)

#### R3.1: The spend row sums both cache counters
- **Given**: `total_cache_read: 1_900_000` and `total_cache_creation: 12_000`
- **When**: The spend row is composed
- **Then**: It shows `cache 1.9M` — their sum, abbreviated — under the `(billed)` qualifier

#### R3.2: Cumulative totals never appear as a percentage
- **Given**: Any snapshot
- **When**: The header is composed
- **Then**: The only percentage is `context N%`, computed from `context_used`; no cumulative counter is divided by the window

### R4: Uptime and idle (F4)

#### R4.1: Both derive from the injected clock
- **Given**: `first_event_ms` 45m12s before and `last_event_ms` 2m03s before a fixed `now_ms`
- **When**: The activity row is composed
- **Then**: It shows `up 45m12s · idle 2m03s`

#### R4.2: The staleness caveat is always present
- **Given**: Any snapshot with event times
- **When**: The activity row is composed
- **Then**: It carries the `(as of last update)` qualifier, since nothing recomposes the overlay on a timer

#### R4.3: Missing event times omit the values
- **Given**: A snapshot whose `first_event_ms`/`last_event_ms` are `None`
- **When**: The activity row is composed
- **Then**: It shows `—`, never an epoch-derived duration

### R5: Purity and the composer contract (F5, F6)

#### R5.1: The composer is a pure function
- **Given**: The public entry point in `dashboard.rs`
- **When**: Its signature and body are inspected
- **Then**: It takes `(&TelemetrySnapshot, now_ms)` and returns `Vec<String>`, reading no clock, no filesystem and no global

#### R5.2: The same inputs always produce the same output
- **Given**: One snapshot and one `now_ms`
- **When**: The composer runs twice
- **Then**: Both outputs are byte-identical, which is what makes the `insta` snapshots stable

#### R5.3: `dashboard.rs` imports no renderer or windowing type
- **Given**: The module
- **When**: Its imports are inspected
- **Then**: No `winit`, `wgpu` or `renderer` type appears

### R6: Empty and degraded states (F7)

#### R6.1: The empty header matches its pinned form
- **Given**: `latest: None` (no log resolved)
- **When**: The composer runs
- **Then**: The output equals F3a's empty example — the frame renders with `—` in every value slot and no `context` label

#### R6.2: A zero context gauge omits the percentage
- **Given**: A known model but `context_used: 0`
- **When**: The header is composed
- **Then**: No `context` label appears — a fresh session's occupancy is unknown, not zero

### R7: Wiring and gates (F8, Constraints)

#### R7.1: `main.rs` gains exactly one line
- **Given**: The diff
- **When**: `main.rs` is inspected
- **Then**: The only change is `mod dashboard;` in the existing alphabetical run

#### R7.2: `dashboard.rs` is a single file
- **Given**: The tree after this feature
- **When**: `crates/app/src/` is listed
- **Then**: `dashboard.rs` exists as a file; there is no `dashboard/` directory

#### R7.3: The clock conversion cannot panic
- **Given**: The `now_ms` call site in `overlay.rs`
- **When**: It is inspected
- **Then**: The `SystemTime`/`u128` conversion is saturating with a documented fallback and contains no `unwrap`

#### R7.4: Workspace gates stay green
- **Given**: The workspace after this feature
- **When**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` run
- **Then**: Both pass, and `insta` appears only under `[dev-dependencies]` pinned to the workspace's existing 1.48.0

## Tasks

### T1: Module skeleton, composition helpers and wiring [infra]
- **Fulfills**: R1, R5, R7.1, R7.2, R7.3
- **Depends on**: (none)
- Create `crates/app/src/dashboard.rs` with `mod dashboard;` in `main.rs`; define `RULE_COLS = 104`, the open-right rule helper and the 8-column label gutter; expose the pure `(&TelemetrySnapshot, now_ms) -> Vec<String>` entry point; reduce `overlay.rs` to wiring with a saturating `now_ms`.

### T2: Model table, context gauge and spend row [vertical]
- **Fulfills**: R2, R3
- **Depends on**: T1
- Add the longest-prefix window table seeded from the measured ids, the omit-on-unmatched rule, the `context N%` computation, and the spend row with `cache = read + creation` under `(billed)`.

### T3: Activity row [vertical]
- **Fulfills**: R4
- **Depends on**: T2
- Compute uptime/idle from the injected clock with saturating subtraction, the `(as of last update)` qualifier, and `—` when event times are absent.

### T4: Empty-state handling [vertical]
- **Fulfills**: R6
- **Depends on**: T3
- Render the full frame with `—` placeholders when the snapshot is absent or its fields are unset, omitting `context` rather than showing a zero.

### T5: Snapshot tests and gates [infra]
- **Fulfills**: R0, R7.4
- **Depends on**: T4
- Add `insta` under `[dev-dependencies]` pinned to 1.48.0; pin F3a's populated and empty examples as snapshots over hand-built `TelemetrySnapshot` values at a fixed `now_ms`; run the workspace gates.

## External Dependencies

### Pre-work
- `telemetry-watcher` must be merged first — this feature reads `latest: Option<Arc<TelemetrySnapshot>>` from `overlay.rs` (the `depends_on` edge).

### Post-work
- Eyeball via `/run-term` at more than one window size; F1's `RULE_COLS` and the panel fraction are the two constants most likely to want adjusting after a first look.

## Known Gaps
- **A 1M-context session is indistinguishable from a 200k one in the log.** This is the real, unfixed problem behind F2. `message.model` carries no window information — a 1M-context run logs as bare `claude-opus-5`, identical to a 200k run — so `context 59%` may in truth be `context 12%`, a 5× over-report, with nothing in the data to detect it. F2a limits the damage to *known* prefixes only, but for those the risk stands. The clean fix is a user override (`[ai_monitor] context_window` in the TOML config, which already live-reloads), and `crates/common/src/config.rs` + `crates/app/src/settings.rs` are **outside this feature's declared surface** — so it is a follow-on slice, not something to smuggle in here.
- **The dashboard is width-blind** (F1), and open-right sections mitigate rather than solve it: the app still cannot right-align, centre, or draw a closed box. Note the two blockers are different — viewport *width* is genuinely unpublished, while the freshness problem below would defeat a width channel even if one existed.
- **Uptime and idle are stale between snapshots** (F4). Nothing recomposes the overlay on a timer or on resize, so both freeze until the next log append. The `as of last update` framing makes this honest rather than fixing it; a real fix needs a periodic overlay poll in `terminal_app.rs` (out of surface).
- **The composed height is not adaptive.** The app *can* read `viewport_h`, `cell` and `scale` from the sidebar's published `SidebarGeom` (`renderer::sidebar_row::TabModelHandle::shared()`, already called at `main.rs:72`), so a line budget is technically derivable today — but it would be stale after resize for the same reason as F4, and `overlay-dashboard-boxes` owns section sizing. Recorded here because it is the concrete follow-on that would let the dashboard fit any window instead of a nominal one.
- **The context-window table will drift** as models ship; `<synthetic>`, `opus` and `claude-fable-5` already match no prefix and so show no percentage (F2a).
