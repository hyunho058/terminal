# loop.md — Loop Contract: sidebar-chrome-collapse

> Supervisor contract for the sidebar-chrome-collapse feature. Batch-approved via /build-order.
- pre-approved-unattended: yes    # batch-approved at build-order Phase-2 gate (2026-07-20)
- spec: specs/sidebar-chrome-collapse/spec.md

## Goal
Draw the sidebar's chrome around the card list — a header row (new-tab + hide buttons; filter drawn but inert), overflow **scroll+clip** of the card list, and a collapsed-state floating restore button over the terminal — with all geometry exposed as **pure layout/hit-test functions that are exact inverses of what is drawn** (SD2), for `sidebar-interaction` to call. Exactly as `specs/sidebar-chrome-collapse/spec.md` specifies.

## Scope & non-goals
- In: `crates/renderer/src/sidebar_header.rs`, `crates/renderer/src/sidebar_layout.rs`, and the single `mod` line(s) in `crates/renderer/src/lib.rs` the spec enumerates.
- Out: event handling / `set_left_inset` changes / collapse-state wiring (sidebar-interaction); the card layout + tab view-model (sidebar-row-redesign — extend additively, never redefine); `term-core`.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| tests | `cargo test --workspace` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --check` |
| typecheck | N/A — Rust; covered by `cargo build` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| clippy warnings | `= 0` | Gate-1 clippy |
| layout↔hit-test parity (SD2) | every region (header buttons, scrolled card rows, collapsed restore button) draws and hit-tests via the same pure function; exact-inverse unit tests pass | `cargo test --workspace` |
| overflow reachability | with more tabs than fit, scrolling brings every tab into the clipped viewport (no tab permanently unreachable) | unit test on the scroll/clamp function |
| per-frame GPU allocation | `= 0` `create_buffer_init` on the record path | code review |
| `unwrap()` outside tests | `= 0` new | grep touched files |

## Gate 3 — Qualitative  (score + grounds + action)
- **Architecture fit** — reuses foundations' primitive + icons; the restore-button layer's z-order over the terminal is deliberate; consumes sidebar-row-redesign's model additively.
- **Naming / readability** — the pure functions `sidebar-interaction` will call are named and documented as the single source of layout truth.
- **Spec alignment** — what fills the header without a search field, header-pinned-vs-scrolls, the scroll model + clamping + active-auto-scroll, and the yours/theirs (geometry vs event) boundary all match the spec.
- **Visual fidelity** — after `/run-term`, header + collapse + overflow match the mock's tokens within the SD1 bar.

---

## Autonomy Boundary
**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs, behaviour-preserving refactors.
**🛑 STOP — escalate:** adding a crate dependency; redefining sidebar-row-redesign's tab view-model instead of extending it; performing the app-side event wiring / `set_left_inset` change that belongs to sidebar-interaction; `term-core` or winit-in-renderer; anything conflicting with the Goal or spec. (If the depended-on sibling's model shape differs from what this spec assumed, escalate — do not silently re-model.)

## Exit criteria
Every gate passes and the Evidence Report is emitted (include a `/run-term` note). A 🛑 boundary ends the loop in escalation.

---

## Lessons
<!-- A fresh contract has none. -->
