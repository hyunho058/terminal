# loop.md — Loop Contract: idle-cpu

> Supervisor document for the idle-cpu feature. Approved in one batch by
> /build-order (post-pr2-backlog). Governs what "done" means so the loop verifies
> itself. Spec: specs/idle-cpu/spec.md.

- pre-approved-unattended: yes

## Goal
Cut idle CPU: the renderer skips re-shaping the grid and re-presents cached
buffers when the `GridSnapshot` is unchanged, and the app re-snapshots only on a
real content change — while the continuous-redraw loop stays (winit#2640). Must
not violate specs/idle-cpu/spec.md (SD1–SD4, D5–D7, R0–R4).

## Scope & non-goals
- In: `crates/renderer/src/lib.rs`, `crates/app/src/terminal_app.rs`.
- Out: `crates/term-core/**` (incl. `snapshot.rs` — `GridSnapshot` already derives
  `PartialEq`, no change needed), other `renderer/` files, the VT color/attr work.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --check` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| cache-hit skips reshape | builders NOT re-invoked on an equal snapshot | a headless test: render/draw twice with an equal `GridSnapshot`, assert the instance-build path ran once (a call counter / instrumented flag) — covers R1.1/R1.4 |
| new-test coverage of the cache | R1.1, R1.2, R1.3, R1.4 each exercised | `cargo test --workspace` (new tests in `renderer`) |
| idle CPU % | ~65% → low idle (goal, not an automated gate) | MANUAL post-merge: `top -pid <app>` / Activity Monitor at a static prompt — recorded as R0 evidence, NOT a loop-blocking command |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — cache keyed on `GridSnapshot ==` (SD2); resident buffers on `Renderer`; zero `term-core` change (SD1).
- **Spec alignment** — R1–R4 met, incl. the invalidation cases: `apply_config`/`resize` reset the renderer cache (R1.4) and the app re-snapshots on tab-switch/resize/config-reload, not only PTY advance (R3.3–R3.5).
- **No regression** — continuous `request_redraw` cadence and the `Occluded` pause are byte-behaviorally identical (SD4); a cached frame is pixel-identical to re-shaping the same snapshot.
- **Naming / readability** — the `snapshot_dirty` flag reads distinctly from the SD2-rejected renderer dirty-flag (per D7's note).

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the cache/resident-buffer logic in `lib.rs`, the
`snapshot_dirty` handling in `terminal_app.rs`, in-scope tests, lint/fmt, local
refactors that preserve behavior.

**🛑 STOP — call a human (escalate):**
- Any change that would require editing `crates/term-core/**` (incl. `snapshot.rs`)
  or pulling a GPU/winit type toward `term-core` (SD1 boundary).
- Disabling / throttling the continuous redraw or the occluded pause (SD4 — would
  regress winit#2640).
- Anything conflicting with this contract's Goal or specs/idle-cpu/spec.md.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop
in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- A fresh contract has none. -->
