# loop.md — Loop Contract: ai-monitor

- pre-approved-unattended: yes

## Goal
Add a native toggle overlay that reads the active session's Claude JSONL logs (backfill + tail) and renders them, per `specs/ai-monitor/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `crates/ai-monitor/**`, `crates/renderer/src/overlay_layer.rs`, `crates/app/src/overlay.rs`
- Out: everything else. Registers via the SD3 `DrawLayer` + SD4 `Overlay` seams. Read-only on logs (C3).

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
| ai-monitor crate tests | `≥ 3 pass` (resolve log, backfill, malformed-safe) | `cargo test -p ai-monitor` |

## Gate 3 — Qualitative
- **Architecture fit** — `ai-monitor` crate is UI/GPU-free (SD2); overlay is a registered `Overlay` (SD4) + `DrawLayer` (SD3); read-only, never writes logs or PTY.
- **Spec alignment** — R0–R3 GWTs hold (backfill on open, tail, visibility-gated watcher, follows active tab, read-only).
- **Naming / readability** — the `LogWatcher` / `AiEvent` model reads clearly.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the declared surface above (incl. adding the `ai-monitor` crate to the workspace).
**🛑 STOP — escalate (park):**
- Editing files outside the declared surface (esp. renderer `lib.rs`, app `terminal_app.rs`).
- Any write to a session log file or the PTY (read-only invariant C3).
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.
