# Performance targets & gates

> **STATUS: PROVISIONAL — numbers NOT yet human-confirmed (gap G1 open).**
> The thresholds below are *candidates* seeded from the requirements interview
> examples plus measured baselines. They are the single source of truth the
> `perf.yml` gate reads (D5/C4), but the gate stays **advisory** until a human
> confirms the committed numbers here and flips the marker. See the park entry
> for `performance-harness` in `build_order.md`.
>
> **Update:** the idle-CPU debt from PR #2 (a full grid re-shape every frame while
> idle) has been fixed — the renderer skips re-shape on an unchanged snapshot. The
> *threshold numbers* below are still provisional (G1 stays open); only the idle
> re-shape hot path changed, not the committed targets.

## Machine class

- **Baseline machine**: Apple M1 (arm64), `--release`, `codegen-units=1`, thin LTO.
- **CI runner**: GitHub `macos-15` (Apple Silicon; `macos-14` until its
  2026-11-02 retirement). Runner timings run slower and noisier than a warm
  local M1 — hence the tolerance band (below).

## The three targets

Each maps to one Criterion bench group (`benches/`, D2) and one interview goal.

| # | Target (goal) | Bench group | Measured baseline (M1, median) | Provisional threshold | Confirmed? |
|---|---------------|-------------|-------------------------------|-----------------------|------------|
| 1 | **input→screen ≤ 8 ms** (CPU parse portion) | `input_to_grid` | **≈ 1.02 µs** / prompt-echo `advance` | ≤ **50 µs** (CPU budget; GPU/present tail measured on runner) | ❌ pending |
| 2 | **120 fps sustained** (≤ 8.33 ms/frame) | `frame` | **≈ 741 µs** / full-screen 2560×1440 submit (dense text) | ≤ **8.33 ms** / full-screen frame submit | ❌ pending |
| 3 | **100k-line scroll, no frame drops** | `scroll_100k` | **≈ 10.4 µs** / `scroll_up(40)+snapshot` over ~100k rows | ≤ **200 µs** / scroll-page + snapshot | ❌ pending |

### Notes per target

1. **input→grid** — measures `Terminal::advance` of a colored prompt + typed
   command + CRLF. This is only the CPU parse→grid slice of the ≤ 8 ms
   input→screen budget; the input-event → PTY-write → wake → GPU-present tail is
   not captured by this bench (it needs the on-runner end-to-end timing in
   `perf.yml`). Baseline ≈ 2.1 µs leaves the whole millisecond-scale budget to
   I/O + GPU.
2. **frame** — `1 / frame_time` = fps. 120 fps ⇒ ≤ 8.33 ms/frame. Measured via
   `Renderer::new_headless` (offscreen device, no window) drawing a dense
   full-screen 2560×1440 snapshot; baseline ≈ 741 µs is the CPU-submit portion
   (instance build + pass record + submit), well inside the 8.33 ms budget. The
   GPU-execute + present tail is measured on the runner in `perf.yml`.
3. **100k-scroll** — builds a 100k-line scrollback (the `scrollback` ring), then
   times a `scroll_up` page + `snapshot`. The point is *bounded per-frame work*:
   the baseline ≈ 9.7 µs and does not grow with history depth (the snapshot
   composes a fixed viewport, not the whole buffer). History build for 100k lines
   ≈ 112 ms (one-time, not per-frame).

## Tolerance band

CI-runner variance (gap in Known Gaps) means an absolute equality gate would be
flaky. The comparison step fails only when `measured_median > threshold * (1 +
TOLERANCE)`. **Provisional `TOLERANCE = 0.50`** (50%) — wide, because it is
seeded from a single local machine, not yet from observed `macos-15` baseline
noise. Narrow it once real runner baselines exist.

## Machine-readable thresholds (the gate parses this block)

`perf.yml`'s comparison step reads the fenced JSON below — this keeps the
committed numbers here (C4 single source of truth) while staying parseable.
`threshold_ns` is the absolute median ceiling; `gated: false` would exclude a
group from failing the workflow (all three are gated now). While `confirmed:
false` the whole gate is advisory regardless (reports, never fails).

```json
{
  "confirmed": false,
  "tolerance": 0.50,
  "groups": {
    "input_to_grid": { "threshold_ns": 50000,  "gated": true, "baseline_ns": 1020 },
    "frame":         { "threshold_ns": 8330000, "gated": true, "baseline_ns": 741350 },
    "scroll_100k":   { "threshold_ns": 200000,  "gated": true, "baseline_ns": 10450 }
  }
}
```

## How the gate works (D5)

1. `perf.yml` runs `cargo bench` on `macos-15` with Criterion's JSON output.
2. A comparison step parses each group's `estimates.json` median (ns).
3. It reads the **committed** thresholds from *this file* (not a
   `target/criterion` baseline — those are per-runner and lost between runs).
4. It fails the workflow if any `median > threshold * (1 + TOLERANCE)`.
5. All three groups are gated; the whole gate is **advisory** (reports, never
   fails) while `confirmed: false` — flip to `true` once a human commits the
   numbers.

## Confirming the numbers (the open G1 decision)

To make this file authoritative, a human must:

1. Decide the committed thresholds per target (or accept the provisional ones).
2. Confirm the machine class the numbers are pinned to.
3. Confirm the tolerance band width from observed `macos-15` noise.
4. Decide whether `perf.yml` is **required/merge-blocking** (D4) or **advisory**
   — recorded explicitly, never silently.

Then flip the STATUS marker at the top of this file from PROVISIONAL to CONFIRMED
and un-park `performance-harness` in `build_order.md`.
