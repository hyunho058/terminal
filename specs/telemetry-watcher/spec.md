# Spec: telemetry-watcher

## Meta
- **Created**: 2026-07-28
- **Type**: dev
- **Status**: ready
- **Set**: ai-status-dashboard (`specs/ai-status-dashboard/partition-manifest.md`)
- **Mode**: batch (`pre-approved-batch: yes`) — L0/L2/L3/L4 human prompts skipped under decompose's single partition gate; the L2-reviewer, coverage, GWT and DAG gates all ran
- **Approved by**: decompose partition gate (`specs/ai-status-dashboard/partition-manifest.md`)
- **Approved at**: 2026-07-28

## Declared Surface
- feature-id: telemetry-watcher
- depends_on: [telemetry-model]
- declared-surface:
  - crates/ai-monitor/src/lib.rs
  - crates/app/src/overlay.rs

## Goal
Refactor `LogWatcher` (`crates/ai-monitor/src/lib.rs:144`) so the watcher thread owns a `SessionTelemetry` and sends **snapshots** instead of per-line `AiEvent`s — today it parses each line to an `AiEvent` and discards the raw line, so there is nothing downstream to accumulate from. Keep the UI thread cheap and match the existing reader-thread → channel → `EventLoopProxy` model.

## Non-goals
- Any dashboard layout — `overlay.rs` gets a minimal placeholder rendering here; `overlay-dashboard-frame` replaces it
- Changing the telemetry fold itself (that is `telemetry-model`)
- Watching the `subagents/` sibling directory (a second watch target — see Known Gaps)
- Editing `terminal_app.rs`, `seam.rs` or the renderer — all outside this feature's declared surface

## Confirmed Goal
`LogWatcher::spawn` owns a `SessionTelemetry`, folds every newly-appended complete line into it, and sends one immutable `TelemetrySnapshot` per append batch on its `mpsc` channel, waking the loop exactly as before. A new telemetry-aware backfill seeds that state from the existing file tail. `AiMonitorOverlay` holds the latest snapshot instead of a `Vec<AiEvent>`, drains the channel keeping only the newest snapshot, and renders a placeholder summary. No JSON parsing remains on the UI thread.

## Research
- `LogWatcher::spawn` already does the hard parts — dir watch, offset tracking, partial-line buffering, shrink/rotation detection, `wake()` once per batch (`crates/ai-monitor/src/lib.rs:155-217`). Only the *payload* changes: `tx.send(ev)` per parsed line becomes one snapshot per batch.
- The offset is advanced by **bytes actually read**, not the pre-read `size`, and a lossy decode survives a multibyte split — both deliberate fixes with a regression test (`crates/ai-monitor/src/lib.rs:186-197`, test at `:279-306`). The refactor must not regress them.
- Shrink/rotation is already detected (`size < offset` → `offset = 0`, clear partial, `:179-183`) but currently only rewinds the read position — there is no accumulated state to reset. With telemetry there now is.
- `backfill` is called **only** by `crates/app/src/overlay.rs:59`; `tabmeta` uses `activity::*`, `resolve_log_in`, `encode_cwd` and `claude_projects_dir`, never `backfill` (verified by grep across `crates/`). `parse_line` *is* used by `activity.rs:82,156`.
- The wake path is shared: `UserEvent::OverlayPending` is emitted by both this watcher and the tab-metadata worker, and handled once at `crates/app/src/terminal_app.rs:1138-1146`, which calls `overlays.poll_all()`. Both `UserEvent` and that handler live in `terminal_app.rs` — **outside this feature's declared surface** — so the existing enum variant and `Overlay::poll` seam must be reused unchanged.
- `Overlay::poll(&mut self) -> bool` returns "did anything change" to drive a redraw (`crates/app/src/seam.rs:139-141`); `AiMonitorOverlay::poll` currently drains the channel into a `Vec` and re-renders (`crates/app/src/overlay.rs:118-135`).
- Visibility-gated lifecycle already exists: `open()` resolves the log, backfills and spawns; `close()` drops the watcher; `on_active_changed` re-opens on a real cwd change (`crates/app/src/overlay.rs:52-75,137-142`).
- `OverlayHandle` is `Rc<RefCell<..>>` (`crates/renderer/src/overlay_layer.rs:31`) — **not** `Send`. The snapshot must therefore cross the thread boundary through the channel and be written into the handle on the UI thread, never shared with the watcher thread.
- Log-format ground truth (usage replication, timestamps, tool correlation, the `subagents/` sibling directory) is established in `specs/telemetry-model/spec.md` › Research and is not repeated here.

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

### W1: The channel payload becomes `Arc<TelemetrySnapshot>`, one send per append batch
- **Status**: resolved
- **Rationale**: `LogWatcher.rx` changes from `Receiver<AiEvent>` to `Receiver<Arc<TelemetrySnapshot>>`. The watcher folds **every** complete line in the batch into its owned `SessionTelemetry`, then sends **one** snapshot and calls `wake()` once — matching the existing once-per-batch wake (`lib.rs:198-209`).
  **Why `Arc`, decided now rather than deferred.** The honest cost model is per-`notify`-event, and Claude Code appends line-by-line, so the steady state is a stream of **single-line batches** — "one send per batch" is in practice "one send per line". A bare `TelemetrySnapshot` would then deep-clone ~112 ring entries plus a 64-entry tasks map of owned `String`s on *every appended line*, hundreds of allocations each, with all but the last discarded by W4. That is a steady-state regression against the old one-small-`AiEvent`-per-line design, not a burst-only one. `Arc` makes the send a refcount bump, still satisfies `telemetry-model` R9.1 (owned, `Clone`, `Send`, borrow-free — `Arc<T>: Send + Clone` given `T: Send + Sync`, which W2a pins), and leaves W7's `!Send`-`OverlayHandle` argument untouched.
  **This is not the `Arc` SD5 rejects.** SD5 rules out an `Arc<Mutex<SessionTelemetry>>` — a *shared mutable* accumulator behind a lock on the render path. This is an immutable per-batch snapshot with no lock and no shared mutation: the watcher still owns the only `SessionTelemetry`, and what crosses the channel is a frozen view. A reader meeting the word `Arc` on both sides deserves that distinction spelled out. Deciding it here is also strictly cheaper than later: `overlay-dashboard-frame` reads `latest: Option<Arc<TelemetrySnapshot>>`, and its composer takes `&TelemetrySnapshot`, which `Arc` derefs to unchanged — so no sibling signature moves.
  Rejected: a bare `TelemetrySnapshot` on the channel (deep clone per appended line, as above); sending per line (N× for one visible state); sending the raw line and folding app-side (SD5 — moves parsing to the UI thread); sending a delta (the snapshot is already bounded by the D6 caps, and deltas need reconciliation logic the UI does not have).

### W2: A new `backfill_telemetry`, run by the caller on the UI thread — as `backfill` is today
- **Status**: resolved
- **Rationale**: Opening the overlay must seed the accumulator from what is **already** in the file — `notify` signals only future appends — so `backfill_telemetry(log) -> (SessionTelemetry, u64)` folds the whole file and returns the byte offset to tail from, mirroring `backfill`'s contract (`lib.rs:129-140`).
  **It runs where `backfill` runs: in `open()`, on the UI thread**, and `spawn` takes the seeded `SessionTelemetry` and offset as parameters. This is the status quo — `overlay.rs:59` already reads and JSON-parses the entire file on the UI thread at open, so folding instead of building `AiEvent`s is the same order of cost, not a new one. Moving the seed into the watcher thread would be a *separate* improvement, and it changes W5's semantics (`latest` would be `None` until the first wake, rather than `Some` immediately after `open()`); mixing that into a payload refactor is how a regression gets in unnoticed.
  **`backfill` is kept**, and for a concrete reason rather than deference: it is `pub` in a lib crate, so it raises no `dead_code` warning under `-D warnings`, its existing test (`lib.rs:255-276`) keeps passing, and removing it is churn for zero gain. (`telemetry-model`'s Constraints also pin it, but justify the pin with "`tabmeta` depends on them" — which is **false for `backfill` specifically**: `tabmeta` uses `activity::*`, `resolve_log_in`, `encode_cwd` and `claude_projects_dir` only. The pin's conclusion is right; its stated reason is over-broad and should not be propagated.)
  Rejected: repurposing `backfill`'s name and signature (breaks its existing test for no gain); seeding inside `spawn` (a real option, but it changes W5's `None` semantics and belongs in its own slice — see Known Gaps).

### W2a: `SessionTelemetry: Send` + constructible, and `TelemetrySnapshot: Sync`
- **Status**: resolved
- **Rationale**: Three dependencies on the sibling's types that are otherwise unstated, all auto-derived today and therefore all liable to break silently later.
  - **`SessionTelemetry: Send`** — W2 moves one into the watcher thread and W3 constructs a fresh one there.
  - **`SessionTelemetry: Default` / `new()`** — W2 builds one in `backfill_telemetry`, W3 replaces one on reset. `telemetry-model` D11 and T1 enumerate `apply` and `snapshot` but name no constructor.
  - **`TelemetrySnapshot: Sync`** — required, and easy to miss: `Arc<T>: Send` holds only when `T: Send + Sync`, so W1's channel payload needs `Sync` on top of the `Clone + Send` that `telemetry-model` R9.1 already pins. Pinning `Send` on one type and not `Sync` on the other would be an arbitrary asymmetry in exactly the place this decision exists to close.
  All three are additive to `telemetry-model`'s surface and cost it nothing. Rejected: leaving them implicit (a later change adding an `Rc` cache or a `Cell` counter to either type would break this feature with no spec-level signal — and the `Arc`/`Sync` coupling in particular fails at a call site far from the cause).

### W3: Rotation and shrink reset the telemetry — and the reset itself emits
- **Status**: resolved
- **Rationale**: `lib.rs:179-183` already detects `size < offset` and rewinds to 0. Until now there was no accumulated state, so a rewind was sufficient; with telemetry, continuing to fold a *different* file's lines into the previous session's totals would silently produce nonsense — tokens from two sessions summed, a `first_event_ms` from the old one. On shrink/rotation the watcher therefore replaces its `SessionTelemetry` with a fresh one before re-reading, mirroring the TS reference's `switchFile` (`logTailer.ts:133-146`).
  **The send condition must widen, or the reset is invisible.** Today the wake fires only when `emitted` is set, and `emitted` is set only when a line parsed *and* sent (`lib.rs:198-209`). A rotation typically truncates to zero first, so the callback fires, `size < offset` resets the state, the read yields zero complete lines, `emitted` stays false — **no snapshot, no wake**, and the UI keeps rendering the previous session's totals as though live until some later line happens to arrive. Under the old `AiEvent` feed this was invisible (there was no state to be stale); with an accumulator it is a visibly wrong number on screen. The condition therefore becomes **"≥1 line folded **or** a reset occurred"**. For completeness the third case is pinned too: bytes read but no complete line (a partial buffered by W8) folds nothing and sends nothing, which is correct — no state changed.
  Rejected: keeping the accumulator across a rotation (cross-session contamination); resetting without emitting (the bug above); tearing down and respawning the watcher (the dir watch and thread are still valid — only the state is stale).

### W4: `poll()` drains the channel and keeps only the newest snapshot
- **Status**: resolved
- **Rationale**: Snapshots are **cumulative**, not incremental, so every one but the last is already stale by the time the UI thread wakes. `poll` therefore drains with `try_recv` until empty, retains only the final value, updates the model once and returns `true` if anything arrived — one model write and one redraw per wake regardless of how many batches queued up during a burst. Rejected: rendering each drained snapshot (N redraws for one visible state); `recv()` (blocks the UI thread); keeping a queue (there is no history to show — the snapshot *is* the history).

### W5: `AiMonitorOverlay` holds `Option<Arc<TelemetrySnapshot>>` and renders a documented placeholder
- **Status**: resolved
- **Rationale**: `events: Vec<AiEvent>` and `MAX_EVENTS` are replaced by `latest: Option<Arc<TelemetrySnapshot>>`, matching W1's channel payload.
  **`None` means exactly one thing: no log was resolved.** The ordering inside `open()` is pinned so this is unambiguous — call `backfill_telemetry`, take `.snapshot()` from the seeded state and store it as `Some(Arc::new(..))` in `latest`, *then* move the `SessionTelemetry` into `spawn`. Because W2 seeds on the UI thread, `latest` is therefore `Some` immediately after a successful `open()`, never "opened but not yet folded"; leaving it `None` until the first wake would be the seeded-inside-spawn behaviour W2 explicitly defers.
  `refresh_model` composes a handful of `key: value` lines from the snapshot — enough to prove the pipeline end to end by eye. This is **explicitly a placeholder** that `overlay-dashboard-frame` replaces; it is not a layout worth reviewing, and saying so in the code comment prevents a later reader from mistaking it for the intended design. Rejected: leaving the old event-feed rendering (there are no events on the channel any more — it would not compile); shipping the full dashboard here (that is the next two features' declared surface, and this feature must not pre-empt it); rendering nothing (the feature would be unverifiable by eye).

### W6: Reuse the existing wake path and `poll` seam — no `terminal_app.rs` edit
- **Status**: resolved
- **Rationale**: `UserEvent::OverlayPending` and its handler (`terminal_app.rs:1138-1146`, which calls `overlays.poll_all()`) are **outside** this feature's declared surface, and the surface is what the partition gate approved. The refactor needs no change there: the wake signal is already "an overlay has pending background work", which is exactly what a new snapshot is. Keeping the seam untouched is also why the `Overlay` trait's `poll -> bool` contract is preserved verbatim. Rejected: a dedicated `UserEvent::TelemetryPending` variant (edits an undeclared file for no behavioural gain, and the tab-metadata worker already shares this variant); passing the snapshot through the event (winit user events are `Send`-bounded and this couples the enum to `ai-monitor`).

### W7: The watcher thread never touches `OverlayHandle`
- **Status**: resolved
- **Rationale**: `OverlayHandle` is `Rc<RefCell<OverlayModel>>` (`overlay_layer.rs:31`) — **not `Send`**, deliberately, because the renderer's model is UI-thread-owned. The snapshot crosses the boundary through the `mpsc` channel and is written into the handle only from `poll()` on the UI thread. This is a compile-time-enforced invariant, not a convention. Rejected: making `OverlayHandle` `Arc<Mutex<..>>` so the watcher could write directly (widens the renderer's contract — outside this surface — and adds a lock on the render path for no benefit).

### W8: The existing offset, partial-line and lossy-decode behaviour is preserved verbatim
- **Status**: resolved
- **Rationale**: Three subtleties in `spawn` are load-bearing and regression-tested: the offset advances by bytes **actually read** rather than the stale pre-read `size` (or appends between `metadata()` and the read duplicate events), the byte-read + lossy decode survives a multibyte character split where `read_to_string` would error and stall, and a trailing partial line is buffered until its newline arrives (`lib.rs:186-206`). Only the payload construction changes; this machinery is not rewritten.
  The guard test `tail_emits_each_append_exactly_once` (`lib.rs:279-306`) is **retained but its assertions are rewritten**: it currently asserts `e1.summary == "one"`, which no longer type-checks once the channel carries snapshots. It must assert the equivalent property against snapshot fields — that two appends produce two distinct snapshots whose folded state advances, and that no third arrives — preserving the no-duplicate guarantee that is the whole point of the test. "Preserved verbatim" applies to the read-loop behaviour, not to the test body. Rejected: rewriting the read loop while changing the payload (two risky changes at once, and the duplicate-event bug this code fixes is easy to reintroduce); deleting the test because its assertions no longer compile (it guards a fixed bug).

## Constraints
- `crates/app/src/terminal_app.rs`, `seam.rs` and everything under `crates/renderer/` are **outside the declared surface** and must not be edited (partition gate)
- The `Overlay` trait implementation must keep its existing signatures — `priority`, `visible`, `on_key`, `layer`, `poll`, `on_active_changed` (`crates/app/src/seam.rs:125-145`)
- `OverlayModel` stays `{ visible, lines }` (SD3) — this feature writes plain strings into it
- No new crate dependencies (SD6); neither `crates/ai-monitor/Cargo.toml` nor `crates/app/Cargo.toml` is in this feature's surface
- **No app-side snapshot test is added here.** SD7 says app-side composition is `insta`-tested, but `crates/app` has no `[dev-dependencies]` section today and `crates/app/Cargo.toml` is outside this surface — it belongs to `overlay-dashboard-frame`, which introduces both the dev-dependency and the first composed layout worth pinning. An implementer following SD7 literally at this layer would edit an undeclared file
- **`ai-monitor` tests stay in the inline `#[cfg(test)] mod tests` in `lib.rs`** — `crates/ai-monitor/tests/**` is `telemetry-model`'s declared surface, so a new `tests/watcher.rs` would be out of bounds
- **How this feature is verified**, given the two constraints above: `cargo test --workspace` (chiefly W8's rewritten inline guard test, which pins the no-duplicate-append property against snapshot fields), `cargo clippy --workspace --all-targets -- -D warnings`, and a by-eye pass via `/run-term` against W5's placeholder rendering. There is no automated assertion on the app-side output at this layer, and that is deliberate — the placeholder is explicitly not a layout worth pinning
- Read-only on logs: files are opened for reading, never written (existing crate contract, `lib.rs:6`)
- Existing public surfaces `AiEvent`, `parse_line`, `backfill`, `encode_cwd`, `resolve_log*`, `activity::*` keep compiling and passing their tests — `tabmeta` depends on several of them
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` stays clean

## Requirements

### R0: The overlay is driven by folded telemetry, not per-line events (Confirmed Goal, accept-seed)

#### R0.1: Opening the overlay shows state folded from the existing file
- **Given**: A session log already containing assistant usage lines and completed tool pairs, and the overlay hidden
- **When**: The user presses Cmd+Shift+M
- **Then**: The overlay opens and renders values folded from the whole existing file, without waiting for a new append

#### R0.2: A live append updates the overlay
- **Given**: The overlay open on a log
- **When**: A new complete JSONL line is appended
- **Then**: The watcher folds it, sends a snapshot, the loop wakes, and the rendered values reflect the new line

#### R0.3: No JSON parsing remains on the UI thread after open
- **Given**: The implementation after this feature
- **When**: `AiMonitorOverlay::poll` and `refresh_model` are inspected
- **Then**: Neither calls `serde_json` nor `parse_line` — they only read fields off a received snapshot

### R1: The channel carries `Arc<TelemetrySnapshot>`, one send per batch (W1, W2a)

#### R1.1: One append batch produces exactly one snapshot
- **Given**: The overlay open and tailing
- **When**: Three complete lines are appended in a single write
- **Then**: Exactly one snapshot is sent and `wake` is called once, with all three lines folded into it

#### R1.2: The payload is a shared handle, not a deep clone
- **Given**: `LogWatcher`'s channel type
- **When**: Its declaration is inspected
- **Then**: It is `Receiver<Arc<TelemetrySnapshot>>`, so a send is a refcount bump rather than a clone of the rings and tasks map

#### R1.3: The snapshot crosses the thread boundary
- **Given**: `SessionTelemetry` owned by the watcher thread and `TelemetrySnapshot` sent to the UI thread
- **When**: The crate is compiled
- **Then**: It compiles — `SessionTelemetry: Send`, `TelemetrySnapshot: Send + Sync + Clone`, and a constructor exists

### R2: Backfill seeds the accumulator before the watch starts (W2)

#### R2.1: `backfill_telemetry` folds the file and reports the tail offset
- **Given**: A log file of known content
- **When**: `backfill_telemetry(log)` is called
- **Then**: It returns a `SessionTelemetry` folded from every line and the file's byte length as the offset to tail from

#### R2.2: A missing log yields empty state, not an error
- **Given**: A path that does not exist
- **When**: `backfill_telemetry` is called on it
- **Then**: It returns a fresh empty `SessionTelemetry` and offset 0, with no panic

#### R2.3: `open()` seeds `latest` before spawning
- **Given**: A resolvable log for the active tab
- **When**: `open()` runs
- **Then**: `latest` is `Some` immediately — seeded from the backfilled state — and the seeded `SessionTelemetry` is moved into `spawn`

#### R2.4: The pre-existing `backfill` still works
- **Given**: The crate after this feature
- **When**: `cargo test -p ai-monitor` runs
- **Then**: `backfill` still compiles and its existing test passes, unchanged

### R3: Rotation resets the accumulator and emits (W3)

#### R3.1: A truncated file resets the totals
- **Given**: The overlay tailing a log with accumulated token totals
- **When**: The file is truncated to zero and a line from a different session is appended
- **Then**: The rendered totals reflect only the new session — the previous session's tokens are gone

#### R3.2: A reset alone produces a snapshot and a wake
- **Given**: The overlay tailing a log with accumulated state
- **When**: The file is truncated to zero and no complete line follows
- **Then**: A snapshot of the freshly-reset state is still sent and `wake` is called, so the UI stops showing the previous session's numbers

#### R3.3: A partial line changes nothing
- **Given**: The overlay tailing a log
- **When**: Bytes are appended that do not terminate in a newline
- **Then**: No line is folded, no snapshot is sent, and the bytes are buffered for the next append

### R4: `poll` coalesces to the newest snapshot (W4)

#### R4.1: A burst of snapshots produces one model write
- **Given**: Three snapshots queued on the channel while the UI thread was busy
- **When**: `poll` runs
- **Then**: It drains all three, writes the model once from the last, and returns `true`

#### R4.2: An empty channel reports no change
- **Given**: The overlay open with nothing queued
- **When**: `poll` runs
- **Then**: It returns `false` and does not rewrite the model, so no redraw is requested

### R5: The overlay renders from the snapshot (W5, W7)

#### R5.1: A placeholder summary is rendered
- **Given**: A received snapshot with known token totals and event times
- **When**: `refresh_model` runs
- **Then**: `OverlayModel.lines` contains readable `key: value` lines derived from those fields, and `visible` matches the overlay's state

#### R5.2: `None` renders an explicit no-session state
- **Given**: The overlay opened on a tab whose log could not be resolved
- **When**: `refresh_model` runs
- **Then**: `latest` is `None` and the panel shows an explicit "no session" line rather than blank or stale content

#### R5.3: The watcher thread never touches the draw handle
- **Given**: The implementation
- **When**: It is compiled
- **Then**: `OverlayHandle` is written only from `poll`/`open`/`close` on the UI thread — the `!Send` handle is never captured by the watch closure

### R6: The existing seam and read-loop behaviour are preserved (W6, W8)

#### R6.1: No file outside the declared surface is edited
- **Given**: The diff for this feature
- **When**: It is inspected
- **Then**: It touches only `crates/ai-monitor/src/lib.rs` and `crates/app/src/overlay.rs`

#### R6.2: The wake path is reused unchanged
- **Given**: The watcher's `wake` callback
- **When**: A batch is folded
- **Then**: It emits the existing `UserEvent::OverlayPending`, which `terminal_app` already routes to `overlays.poll_all()` — no new event variant

#### R6.3: Appends are never double-counted
- **Given**: The overlay tailing a log
- **When**: Two lines are appended in separate writes
- **Then**: Two snapshots arrive, the second's totals include both lines exactly once, and no third snapshot follows — the byte-accurate offset advance still holds

#### R6.4: A multibyte character split across a read boundary survives
- **Given**: An append whose final bytes split a multibyte character
- **When**: The watcher reads it
- **Then**: The read does not error or stall; the partial is buffered and folds correctly once completed

### R7: Verification (Constraints)

#### R7.1: The guard test is retained with rewritten assertions
- **Given**: `tail_emits_each_append_exactly_once`
- **When**: The suite runs after the refactor
- **Then**: It still exists, asserts against snapshot fields instead of `AiEvent.summary`, and still fails if an append is emitted twice

#### R7.2: No app-side snapshot test and no out-of-surface test file
- **Given**: The diff
- **When**: It is inspected
- **Then**: `crates/app/Cargo.toml` is unmodified, no `insta` usage is added to `crates/app`, and all new `ai-monitor` tests live in the inline `#[cfg(test)] mod tests` in `lib.rs`

#### R7.3: Workspace gates stay green
- **Given**: The workspace after this feature
- **When**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` run
- **Then**: Both pass, including the pre-existing `tabmeta` and `activity` tests

## Tasks

### T1: Channel payload + watcher-owned accumulator [vertical]
- **Fulfills**: R1, R3, R6.2, R6.3, R6.4
- **Depends on**: (none)
- Change `LogWatcher.rx` to `Receiver<Arc<TelemetrySnapshot>>`; have `spawn` take the seeded `SessionTelemetry` and offset; fold every complete line in a batch and send one `Arc` snapshot; widen the emit condition to "≥1 line folded **or** a reset occurred"; replace the accumulator on shrink/rotation. Leave the offset/partial/lossy-decode machinery untouched.

### T2: `backfill_telemetry` [infra]
- **Fulfills**: R2
- **Depends on**: T1
- Add `backfill_telemetry(log) -> (SessionTelemetry, u64)` folding the whole file, missing-file-safe. Keep `backfill` and its test intact.

### T3: Overlay consumes snapshots [vertical]
- **Fulfills**: R0, R4, R5, R6.1
- **Depends on**: T2
- Replace `events`/`MAX_EVENTS` with `latest: Option<Arc<TelemetrySnapshot>>`; seed it in `open()` before `spawn`; drain-to-newest in `poll`; compose the documented placeholder in `refresh_model`, with an explicit no-session line for `None`.

### T4: Tests and gates [infra]
- **Fulfills**: R7, R1.3
- **Depends on**: T3
- Rewrite the guard test's assertions against snapshot fields; add inline tests for the rotation reset (R3.1/R3.2), the partial-line no-op (R3.3) and poll coalescing (R4); confirm `Send`/`Sync`/constructor bounds compile; run the workspace gates.

## External Dependencies

### Pre-work
- `telemetry-model` must be merged first — this feature consumes `SessionTelemetry`, `TelemetrySnapshot`, `Send`/`Sync` bounds and a constructor from it (the `depends_on` edge).

### Post-work
- (none)

## Known Gaps
- **The `subagents/` sibling directory is still not watched.** `telemetry-model`'s Known Gaps records that subagent transcripts live in `<session-uuid>/subagents/agent-*.jsonl` and are excluded from totals. Adding them is a *watcher* concern — a second watch target and a merge policy for two accumulators — and is deliberately out of scope here.
- **A new *session* is a worse staleness case than a rotation, and is not handled.** W3 resets on `size < offset`, but a new Claude Code session writes a **new `<uuid>.jsonl`**: the watched file never shrinks and simply stops receiving appends, while `on_active_changed` re-points only when the *cwd* changes (`overlay.rs:137-142`). The overlay then shows the previous session's totals indefinitely. This is pre-existing behaviour, but it is materially worse under telemetry than under the old feed — a frozen `AiEvent` list reads as "idle", whereas stale token totals and a frozen uptime read as *live*. W3's own premise is that cross-session contamination is unacceptable, so the more likely cross-session failure deserves naming. The fix (re-resolving the newest log on a timer or on a directory-create event) is a watcher change but a distinctly larger one, and would want a decision about what to do with the accumulated state.
- **A `notify` event for a file that is neither appended nor rotated** (e.g. a metadata-only touch) still costs a seek + read of zero bytes and sends no snapshot. Harmless, unchanged from today.
- **A transient `File::open`/`metadata` failure delays a reset** past the rename window (`lib.rs:173-178` return before the shrink check). The next event resets and folds, so the state converges; no reset is lost.
