# loop.md — Loop Contract: telemetry-watcher

- pre-approved-unattended: yes

## Goal
Make the watcher thread own a `SessionTelemetry` and send `Arc<TelemetrySnapshot>`s instead of per-line `AiEvent`s, adapting `overlay.rs` to consume them — per `specs/telemetry-watcher/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `crates/ai-monitor/src/lib.rs`, `crates/app/src/overlay.rs`
- Out: everything else. No dashboard layout (that is `overlay-dashboard-frame`). Read-only on logs (C3).

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| no duplicate appends (W8) | 2 appends → 2 snapshots, **no** 3rd | rewritten `tail_emits_each_append_exactly_once` |
| rotation emits (W3) | truncate-to-zero with no following line still sends a snapshot and wakes | inline test |
| partial line (W3) | bytes without a newline → 0 snapshots | inline test |
| poll coalescing (W4) | 3 queued snapshots → 1 model write, `poll` returns `true` | inline test |
| no UI-thread parsing | `0` occurrences of `serde_json` / `parse_line` in `poll` / `refresh_model` | grep on `overlay.rs` |
| existing surface intact | `backfill`, `parse_line`, `activity::*`, `tabmeta` tests all pass | `cargo test --workspace` |

## Gate 3 — Qualitative
- **Concurrency shape (SD5/W7)** — the watcher owns the only `SessionTelemetry`; an immutable `Arc` snapshot crosses the channel; the `!Send` `OverlayHandle` is written only on the UI thread.
- **Seam reuse (W6)** — the existing `UserEvent::OverlayPending` and `Overlay::poll` are reused; no new event variant, no `terminal_app.rs` edit.
- **Placeholder honesty (W5)** — the interim rendering is commented as a placeholder that `overlay-dashboard-frame` replaces, so no later reader mistakes it for the intended design.
- **Spec alignment** — R0–R7 GWTs hold.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the two declared files, including rewriting the guard test's assertions against snapshot fields.
**🛑 STOP — escalate (park):**
- Editing `crates/app/src/terminal_app.rs`, `crates/app/src/seam.rs`, or anything under `crates/renderer/` (out of surface).
- Editing `crates/app/Cargo.toml` — the `insta` dev-dependency is deferred to `overlay-dashboard-frame`, which owns that file.
- Creating a new `crates/ai-monitor/tests/*.rs` file — that directory is `telemetry-model`'s surface; watcher tests stay inline in `lib.rs`.
- Deleting `backfill`, `AiEvent` or `parse_line` (pinned public surfaces).
- Any write to a session log file or the PTY (read-only invariant C3).
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Appended by the unattended run of 2026-07-28. `source: auto-unattended` — no
     human approved these at record time; review and prune. -->

### L1 — a Gate-2 row is satisfied by a test that RAN, never by a grep
- cause: gate-fail
- source: auto-unattended
- form: gate1-command
- status: active
- check: `cargo test --workspace 2>&1 | grep -q "^test .*<test_name> ... ok"` for every test a Gate-2 row names — assert the name appears in the RUN OUTPUT, not in the source.
- note: iteration 1 reported Gate 2 PASS by counting greps for comment strings. Two rows the contract requires — the R3.3 partial-line and R4 poll-coalescing tests — had no test behind them at all; the greps measured only that a phrase had been written. A verification that inspects the source instead of the run is not a verification.

### L2 — a constraint that forbids one MECHANISM does not forbid the requirement
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: loop.md Gate 2 required an inline poll-coalescing test while spec.md appeared to forbid app-side tests. The constraint was scoped to an *insta* test (it needs a dev-dep in an out-of-surface `Cargo.toml`); a plain `std` `#[test]` engages none of its rationale. Finding a mechanism satisfying both is the correct resolution. The line to escalate at: if satisfying the requirement had needed `poll`'s BEHAVIOUR reshaped rather than a pure extraction, that is the test wagging the design.

### L3 — `crates/app` is a bin-only crate; `--lib` silently runs nothing
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: `cargo test -p app --lib` errors out without running the inline `#[cfg(test)]` tests in `overlay.rs`. Use `cargo test --workspace` (which does pick them up) or `-p app --bins`. A green `--lib` run here proves nothing.
