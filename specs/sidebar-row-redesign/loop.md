# loop.md — Loop Contract: sidebar-row-redesign

> Supervisor contract for the sidebar-row-redesign feature. Batch-approved via /build-order.
- pre-approved-unattended: yes    # batch-approved at build-order Phase-2 gate (2026-07-20)
- spec: specs/sidebar-row-redesign/spec.md

## Goal
Replace the flat one-line sidebar with the mock's rounded two-line **card** (mark + title on line 1, branch icon + branch on line 2), active card highlighted, agent-backed cards carrying their `AgentSkin` gradient/border, on a config-driven-width sidebar — exactly as `specs/sidebar-row-redesign/spec.md` specifies. Reuse foundations' primitives; own the mapping from tab-metadata's agent identity to `AgentSkin` (foundations D13).

## Scope & non-goals
- In: `crates/renderer/src/tab_bar.rs` (rewrite/replace), `crates/renderer/src/sidebar_row.rs`, `crates/app/src/sidebar_model.rs`, `crates/app/src/tabs.rs`, and the minimal edits the spec enumerates in `crates/renderer/src/lib.rs` + `crates/app/src/main.rs`.
- Out: header/collapse/scroll (sidebar-chrome-collapse); click/hover/wheel/inset event wiring (sidebar-interaction); `term-core`; foundations' files.

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
| per-frame GPU allocation | `= 0` `create_buffer_init` on the record path (uses `upload_resident`) | code review of `record` |
| draw/hit-test parity (SD2) | the card/region layout function and its inverse are exact inverses, unit-tested | `cargo test --workspace` (the `slot_y`/`tab_at`-style test) |
| out-of-surface tests still green | `input.rs` etc. compile AND pass | `cargo test --workspace` |
| `unwrap()` outside tests | `= 0` new | grep touched files |

## Gate 3 — Qualitative  (score + grounds + action)
- **Architecture fit** — reuses `SidebarShape`/`SidebarShapePipeline`/`SidebarTheme`/`SidebarMetrics`/`Icon`/`push_icon` from foundations; no reinvented primitive; one `TabBar` layer registered.
- **Naming / readability** — the tab view-model (`TabModelHandle`/`SidebarModel`) is clear and its per-tab branch + agent-identity + selection fields read well; extension points for chrome-collapse are additive.
- **Spec alignment** — the two-text-size decision, card height policy, px-space ellipsis, and the SD2 hit-test-inverse for non-uniform (margin/gap) layout all match the spec's decisions.
- **Visual fidelity** — after `/run-term`, cards match the mock's tokens (13px radius, two lines, mark glyphs, agent skins, active highlight) within the "honor the tokens, not a pixel diff" bar (SD1).

---

## Autonomy Boundary
**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs, behaviour-preserving refactors, deleting the now-dead `tab_bar` internals it replaces.
**🛑 STOP — escalate:** adding a crate dependency; changing foundations' published API rather than consuming it; editing files outside the declared surface (header/collapse/event files belong to later features); `term-core` or renderer-imports-winit; a two-text-size approach that needs a second atlas ppem the renderer can't provide (a real design fork — escalate, don't guess); anything conflicting with the Goal or spec.

## Exit criteria
Every gate passes and the Evidence Report is emitted (include a `/run-term` visual check note). A 🛑 boundary ends the loop in escalation.

---

## Lessons
<!-- A fresh contract has none. -->
