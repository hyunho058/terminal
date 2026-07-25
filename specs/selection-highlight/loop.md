# loop.md — Loop Contract: selection-highlight

- feature-id: selection-highlight
- spec: specs/selection-highlight/spec.md
- pre-approved-unattended: yes   # approved at build-order batch gate 2026-07-24

## Goal
Implement the renderer selection highlight per `specs/selection-highlight/spec.md`: a
hand-rolled per-cell `SelectionLayer` `DrawLayer` (`SELECTION_Z=4`, below the cursor) that
tints `GridSnapshot.selection`, reusing the cell-bg technique through the existing seam —
no GUI crate, no edit to the central `Renderer::draw_to`.

## Scope & non-goals
- In: `crates/renderer/src/{selection_layer.rs, lib.rs}` (the declared surface).
- Out: term-core/app code; the base bg pass / `draw_to`; selection state, mouse, clipboard (siblings).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — `cargo build` is the typecheck in Rust |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| pure instance-builder tests (R2/R4.2) | `≥ 1, all green, no wgpu::Device` | `cargo test -p renderer` |
| test coverage % | N/A — no coverage tooling configured | — |
| fps / latency | N/A as a gate — project GOALS not gates (docs/domain G1) | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — a self-contained `DrawLayer` mirroring `image_layer.rs`; no GUI crate; `renderer` imports no `winit`; the central draw path is untouched.
- **Naming / readability** — mirrors the `ImageLayer` idiom at the call site.
- **Spec alignment** — `SELECTION_Z=4 < CURSOR_Z` with the compile-time guard; translucent alpha-blend (glyphs read through); highlights continuation cells; `None`/empty selection records nothing (R1–R4).
- **Testability** — the selected-cell→instance mapping is a pure, GPU-free function (R0.2).

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs/comments,
behaviour-preserving refactors — all WITHIN the declared surface.

**🛑 STOP — call a human (escalate):**
- Adding any GUI/widget crate, or importing `winit` into `renderer` (no-gui-crates / C1).
- Editing `Renderer::draw_to` / the base bg pass (spec forbids central-path edits).
- Editing `Cargo.lock` (PreToolUse hook blocks it).
- Adding a `#[cfg(target_os = ...)]` branch (macos-only rule).
- Editing any file OUTSIDE the declared surface.
- Anything conflicting with the spec's Decisions/SDs or this Goal.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- A fresh contract has none. -->

### L1 — `assert!` on all-const operands trips clippy
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: A test `assert!(SELECTION_Z > 0 && SELECTION_Z < CURSOR_Z)` with only const operands fails `clippy::assertions_on_constants` under `-D warnings`. Assert via a runtime value (e.g. `let z = layer.z_order();`) or use a `const { assert!(..) }` block; the compile-time `const _` guard already covers the pure-const invariant.
