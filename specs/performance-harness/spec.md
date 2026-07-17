# Spec: performance-harness

## Meta
- **Created**: 2026-07-17
- **Type**: dev
- **Status**: drafting
- **Set**: mac-terminal-v1 (`specs/mac-terminal-v1/partition-manifest.md`)

## Declared Surface
- feature-id: performance-harness
- depends_on: [glyph-rendering, scrollback]
- declared-surface:
  - benches/**
  - .github/workflows/perf.yml
  - docs/performance.md

## Goal
Turn the interview's example performance numbers into confirmed, measured gates: benchmark the hot paths (input→grid latency, frame time / sustained fps, 100k-line scroll), record the agreed targets in `docs/performance.md`, and run the benchmarks in a dedicated CI job that fails when a target regresses.

## Non-goals
- Optimizing the code (this feature measures + gates; fixes belong to the feature that owns the slow path)
- GPU-frame capture / profiling tooling beyond timing the CPU-side hot paths and a headless render loop
- Blocking the main CI job (perf runs as its own workflow to avoid flaky-timing noise on every PR)

## Confirmed Goal
`benches/` holds Criterion benchmarks for the parse→grid→snapshot path and a headless render loop; `docs/performance.md` records the confirmed numeric targets (resolving gap G1); and `.github/workflows/perf.yml` runs them on a macOS runner, comparing against recorded thresholds and failing on regression.

## Research
- `insights.md` lists candidate numbers as **examples, not confirmed**: 120fps sustained, input→screen ≤ 8ms, 100k-line scroll with no frame drops (gap G1 in `scaffold-spec.md`).
- `term-core` is headless-testable (SD2/SD7), so parse→grid→snapshot is benchable without a GPU; the renderer needs a headless wgpu device for frame timing (the `glyph-rendering` smoke path).
- No `benches/` yet; CI (`.github/workflows/ci.yml`) runs fmt/clippy/test/build on `macos-14` — perf gets a sibling workflow.
- Depends on `glyph-rendering` (frame path exists) and `scrollback` (100k-line buffer exists) to have real targets to measure.

## Decisions

<!-- Shared Decisions SD1–SD7 inherited verbatim from the partition manifest -->
### SD1: Stack & platform — Rust, pure wgpu/Metal, macOS ARM64 + zsh only
- **Status**: resolved
- **Rationale**: Requirement (순정 GPU 스택, macOS 전용, zsh 로컬 전용). Hand-rolled cell rendering, no GUI/widget crates (winit + objc2 are the only allowed windowing/platform crates). No cross-platform `#[cfg]`, no SSH/remote, no other shells. Rejected: GUI toolkits (egui/iced — defeat the manual-cell goal), cross-platform abstraction (out of scope).

### SD2: Dependency direction — `common ← term-core ← renderer ← app`
- **Status**: resolved
- **Rationale**: `term-core` (grid, vte parsing, PTY) MUST stay GPU-free so terminal logic is headless-testable; `renderer` is window-agnostic (raw-window-handle, no winit). Every feature preserves this direction. Rejected: letting the renderer own terminal state (couples GPU to logic, kills headless tests).

### SD3: Renderer extension seam — pluggable draw layers, not edits to `renderer/src/lib.rs`
- **Status**: resolved
- **Rationale**: `glyph-rendering` establishes a draw-layer API in `crates/renderer`; every later visual feature (tab bar, image layer, ai-monitor overlay) implements a layer in its OWN module and registers through that API rather than editing the central `Renderer`. Keeps feature surfaces disjoint and makes the renderer feature-extensible. Rejected: each feature editing `lib.rs` (creates an unordered hub collision).

### SD4: App event-loop extension seam — owned by `tabs`, extended via registration
- **Status**: resolved
- **Rationale**: `tabs` owns the winit event loop (`crates/app`) and exposes an input/overlay registration seam. Other app-touching features (clipboard paste, ai-monitor overlay, settings reload) add self-contained modules under `crates/app/` and register through the seam instead of editing `terminal_app.rs`. Rejected: multiple features editing `terminal_app.rs` directly (unordered hub collision).

### SD5: Terminal contract — extend `GridSnapshot` via side-channel layers, reuse `vte`
- **Status**: resolved
- **Rationale**: The `term-core → renderer` boundary is `GridSnapshot`. Features add side-channel data (image layer keyed by cell coords, scrollback viewport offset) rather than overloading cell semantics; escape parsing reuses the `vte` crate, grid control stays hand-rolled. Rejected: encoding images/overlays as special cells (pollutes the grid model).

### SD6: Config & concurrency — TOML+serde, threads+channels, event-driven redraw
- **Status**: resolved
- **Rationale**: Config is TOML+serde in `common`. Concurrency is threads + channels (PTY reader thread → channel → UI thread); the render loop is winit-event-driven and woken via `EventLoopProxy` (`ControlFlow::Wait`) — no async runtime. Rejected: tokio/async (overkill for the few I/O sources).

### SD7: Verification & performance gates
- **Status**: resolved
- **Rationale**: Headless `term-core` behavior is tested with `insta` snapshots; GPU paths get a headless smoke test. Performance is judged by numeric targets (candidate: 120fps sustained / input→screen ≤ 8ms / 100k-line scroll with no frame drops) — the exact numbers are OPEN (gap G1) and are confirmed inside the `performance-harness` feature. Until confirmed, other features treat them as goals, not pass/fail gates.

<!-- Feature-local decisions -->
### D1: Criterion for benchmarks
- **Status**: resolved
- **Rationale**: `criterion` gives statistically-robust timings + a saved baseline for regression comparison, the Rust-standard choice. Benches live in `benches/` at the workspace root, exercising the public crate APIs. Rejected: `#[bench]` (nightly-only), hand-rolled timing (no statistical rigor / regression baseline).

### D2: Three benchmark targets map to the three numeric goals
- **Status**: resolved
- **Rationale**: (a) *input→grid*: time `Terminal::advance` of a keystroke-to-echo byte sequence; (b) *frame*: time a headless `Renderer::render` of a full-screen snapshot (fps = 1/frame-time); (c) *scroll*: build a 100k-line history (`scrollback`) then time `scroll_up` + `snapshot` sweeps. Rejected: one mega-benchmark (can't attribute regressions), GPU-vendor capture (not portable/CI-able).

### D3: Confirm the actual target numbers with the human, record in `docs/performance.md`
- **Status**: provisional
- **Rationale**: The 120fps / 8ms / 100k figures are examples (G1). This feature's implementation opens ONE decision to the human — the committed thresholds — and records them (with the measuring method + machine class) in `docs/performance.md`. Until then benches run but the CI gate compares to provisional thresholds. Rejected: hard-coding the example numbers as gates (they were never confirmed — would fail or pass arbitrarily).

### D4: `perf.yml` is a separate workflow that IS a required check (with a tolerance band)
- **Status**: resolved
- **Rationale**: Separate from `ci.yml` (timing noise must not fail functional checks), but to have teeth it **is a required, merge-blocking status check** — a regression beyond the tolerance band blocks the merge, not just paints a dashboard red. The tolerance band (D3/G1) absorbs runner noise so the gate is meaningful without being flaky; if noise proves too high to gate reliably, it downgrades to advisory *explicitly* (a recorded decision), never silently. Rejected: advisory-only (a gate with no teeth — regressions merge and are ignored, defeating the Goal), adding perf to `ci.yml` (scaffold-owned; gates every PR on timing noise).

### D5: The gate reads committed ABSOLUTE thresholds; a script bridges Criterion output
- **Status**: resolved
- **Rationale**: The single source of truth is the absolute thresholds committed in `docs/performance.md` (C4) — not a `target/criterion` baseline (which is per-runner, uncommitted, and lost between fresh CI runs). Criterion produces the *measurement* (`--message-format json` / `estimates.json`); a small comparison step in `perf.yml` parses the measured median and fails if it exceeds the committed threshold + tolerance. This reconciles D1 (Criterion measures) with C4 (absolute gate): Criterion measures, the committed number gates. Rejected: relying on Criterion's built-in baseline compare (no persisted baseline across ephemeral runners; would compare a run to itself), committing `target/criterion` (churny, machine-specific).

## Constraints
- C1: Benchmarks must run headless (no window) — term-core benches need no GPU; the frame bench uses a headless wgpu device.
- C2: This feature only measures/gates; it never edits another feature's source to optimize it (a regression is reported, the owning feature fixes it).
- C3: `perf.yml` must not duplicate or replace `ci.yml`'s functional checks (separate concerns).
- C4: The committed thresholds in `docs/performance.md` are the single source of truth the gate reads.

## Known Gaps
- **G1 resolution**: the confirmed numbers (fps/latency/scroll) are set by D3 during implementation with the human; until then thresholds are provisional.
- CI-runner variance may require a tolerance band; its width is TBD from observed baseline noise.

## Requirements

### R0: Measured performance gates
#### R0.1: Benchmarks run and produce timings
- **Given**: The workspace builds
- **When**: `cargo bench` runs
- **Then**: The three benchmark groups (input→grid, frame, 100k-scroll) each report a timing with a saved Criterion baseline

### R1: Benchmark targets (D1, D2)
#### R1.1: input→grid latency bench
- **Given**: A `Terminal` of default size
- **When**: The input→echo byte sequence is advanced under Criterion
- **Then**: A median time is reported (the input→screen budget's CPU portion)
#### R1.2: frame-time bench
- **Given**: A headless `Renderer` + a full-screen snapshot
- **When**: `render` is timed under Criterion
- **Then**: A median frame time is reported and fps derived (1/frame-time)
#### R1.3: 100k-line scroll bench
- **Given**: A `Terminal` whose scrollback holds 100k lines
- **When**: A scroll-up sweep + `snapshot` is timed
- **Then**: A median time is reported with no allocation-driven blowup (bounded per-frame work)

### R2: Confirmed thresholds (D3)
#### R2.1: Targets recorded
- **Given**: The human has confirmed the numeric targets
- **When**: `docs/performance.md` is written
- **Then**: It states each target (value + how measured + machine class), resolving gap G1

### R3: CI gate (D4, D5)
#### R3.1: Perf workflow fails on regression against committed thresholds
- **Given**: `perf.yml` runs the benches on `macos-14`; the comparison step reads the absolute thresholds from `docs/performance.md`
- **When**: A benchmark's measured median exceeds its committed threshold beyond the tolerance band
- **Then**: The perf workflow fails; a within-threshold run passes
#### R3.2: Perf is independent of functional CI
- **Given**: A PR that changes only docs
- **When**: CI runs
- **Then**: The functional `ci.yml` job and the `perf.yml` job are separate checks (perf timing noise cannot fail functional CI)
#### R3.3: The perf check is merge-blocking
- **Given**: A PR that regresses a benchmark past threshold+tolerance
- **When**: The perf check fails
- **Then**: The PR is blocked from merging (perf is a required status check), unless perf was explicitly recorded as advisory
#### R3.4: Threshold is read from the committed source of truth
- **Given**: `docs/performance.md` records the thresholds
- **When**: The comparison step runs
- **Then**: It parses those committed numbers (not a `target/criterion` baseline) as the gate values

## Tasks

### T1: benches/ with Criterion — the three groups [infra]
- **Fulfills**: R0, R1
- **Depends on**: (none)

### T2: docs/performance.md — confirmed targets (resolve G1) [vertical]
- **Fulfills**: R2
- **Depends on**: T1

### T3: perf.yml workflow + Criterion→threshold comparison step (required check) [vertical]
- **Fulfills**: R3
- **Depends on**: T1, T2

## External Dependencies
### Pre-work
- `glyph-rendering` (headless frame path) and `scrollback` (100k-line buffer) must exist to benchmark (declared dependencies).
- Add `criterion` dev-dependency + `[[bench]]` targets.
### Post-work
- The confirmed numbers (D3) feed back as the acceptance gates other features refer to in SD7.
