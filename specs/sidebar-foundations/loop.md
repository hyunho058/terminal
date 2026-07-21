# loop.md — Loop Contract: sidebar-foundations

> Supervisor contract for the sidebar-foundations feature. Batch-approved via /build-order.
- pre-approved-unattended: yes    # batch-approved at build-order Phase-2 gate (2026-07-20)
- spec: specs/sidebar-foundations/spec.md

## Goal
Ship the config + GPU-primitive + token + icon foundation of the sidebar redesign exactly as `specs/sidebar-foundations/spec.md` specifies, **with zero visible change to the running terminal** (D10, R0.1). Must not violate that spec's decisions or the project rules.

## Scope & non-goals
- In: `crates/common/src/config.rs`, `config.example.toml`, `crates/renderer/src/sidebar_theme.rs`, `crates/renderer/src/sidebar_shapes.rs`, `crates/renderer/src/sidebar_shapes.wgsl`, `crates/renderer/src/sidebar_icons.rs`, `crates/renderer/src/lib.rs` (only the minimal plumbing D10 enumerates).
- Out: `crates/renderer/src/tab_bar.rs` (owned by sidebar-row-redesign; stays registered & unchanged); any `DrawLayer` registration; `term-core`; any per-tab data.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| tests | `cargo test --workspace` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --check` (PostToolUse hook also auto-runs rustfmt) |
| typecheck | N/A — Rust; covered by `cargo build` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| clippy warnings | `= 0` | Gate-1 clippy with `-D warnings` |
| new headless unit tests pass | config (R1–R2), theme (R6), SDF (R5.1), icons (R7), degenerate (R8) all present & green | `cargo test --workspace` |
| GPU-tier tests skip cleanly with no adapter | pass or skip, never fail | run tests; guarded `headless()` returns None → skip |
| `unwrap()` outside `#[cfg(test)]` | `= 0` new | grep the touched files |

## Gate 3 — Qualitative  (score + grounds + action)
- **Architecture fit** — reuses crate-root `build_pipeline`/`QUAD`/`upload_resident` (D11), not a private copy; no GUI crate; renderer imports no winit.
- **Naming / readability** — `SidebarTheme`/`SidebarMetrics`/`SidebarShape`/`AgentSkin` clear at the call site.
- **Spec alignment** — `[sidebar]` fields, defaults, 260–440 clamp-at-load, `SidebarMetrics::scaled`, the WGSL↔Rust SDF cross-check, and D10's exact `lib.rs` diff all match the spec.
- **Visual inertness (R0.1)** — no layer registered, `left_inset` unchanged, `tab_bar` still draws the sidebar; the terminal looks identical before/after.

---

## Autonomy Boundary
**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope unit tests, docs/comments, behaviour-preserving refactors, WGSL↔Rust SDF reconciliation.
**🛑 STOP — escalate:** adding any crate dependency (Cargo.lock is hook-blocked); registering a `DrawLayer` or otherwise changing on-screen output (violates D10/R0.1); editing `tab_bar.rs` or `term-core`; any `#[cfg(target_os)]` branch; anything conflicting with this contract's Goal or the spec.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- A fresh contract has none. -->
