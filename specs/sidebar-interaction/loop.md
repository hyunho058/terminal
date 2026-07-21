# loop.md — Loop Contract: sidebar-interaction

> Supervisor contract for the sidebar-interaction feature. Batch-approved via /build-order.
- pre-approved-unattended: yes    # batch-approved at build-order Phase-2 gate (2026-07-20)
- spec: specs/sidebar-interaction/spec.md

## Goal
Wire all app-side events for the redesigned sidebar — click routes to card-switch / new-tab / hide / restore; collapse returns the reclaimed width to the terminal grid (and expand restores it) keeping grid + PTYs + renderer consistent; hover highlights the card under the pointer; wheel-over-sidebar scrolls the list — by calling the sibling features' **pure layout/hit-test functions** (SD2), never reimplementing layout maths. Exactly as `specs/sidebar-interaction/spec.md` specifies.

## Scope & non-goals
- In: `crates/app/src/input.rs`, `crates/app/src/terminal_app.rs`, `crates/app/src/settings.rs`, `crates/app/src/main.rs`.
- Out: any layout/geometry maths (call chrome-collapse's + row-redesign's pure functions); the renderer's draw code; `term-core`; the tab view-model definition (consume it).

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
| hit-test via shared functions | click/hover dispatch calls chrome-collapse's/row-redesign's pure functions — `= 0` duplicated layout constants app-side (SD2) | code review |
| collapse consistency | after collapse→expand, grid cols, every PTY size, and `left_inset` are mutually consistent; unit-tested on the pure inset/grid math | `cargo test --workspace` |
| scroll delta handles both winit forms | `LineDelta` AND `PixelDelta` mapped | unit test |
| `unwrap()` outside tests | `= 0` new | grep touched files |

## Gate 3 — Qualitative  (score + grounds + action)
- **Architecture fit** — extends the existing `tab_target`/`do_tab_action` dispatch; a new `UserEvent` variant only if justified; no layout maths duplicated.
- **Naming / readability** — the hit-test dispatch order and the collapse call-sequence read clearly.
- **Spec alignment** — hit-test dispatch order + no-region-hit behaviour, hover redraw policy under the continuous-redraw loop, scroll units, collapse persistence + who writes `[sidebar] collapsed`, and the exact collapse call-sequence (grid + PTY + renderer) all match the spec.
- **User-flow naturalness** — after `/run-term`: clicking a card switches, hide/restore works, hover highlights, wheel scrolls, and the shell is resized to the correct size on collapse.

---

## Autonomy Boundary
**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests (pure functions, no `EventLoopProxy` in tests), docs, behaviour-preserving refactors.
**🛑 STOP — escalate:** adding a crate dependency; reimplementing sibling layout maths instead of calling their functions (SD2 violation); a collapse sequence that resizes the shell to the wrong size (getting this wrong corrupts the PTY) — verify against the spec, escalate if unsure; SSH/remote or non-zsh logic; `term-core`; anything conflicting with the Goal or spec. (If a depended-on sibling's pure-function signature differs from what this spec assumed, escalate — do not silently re-derive.)

## Exit criteria
Every gate passes and the Evidence Report is emitted (include a `/run-term` note). A 🛑 boundary ends the loop in escalation.

---

## Lessons
<!-- A fresh contract has none. -->
