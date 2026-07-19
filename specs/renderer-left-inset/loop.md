# loop.md — Loop Contract: renderer-left-inset

> Approved in batch by /build-order (left-tab-sidebar). Gates run by /loop.
- pre-approved-unattended: yes
- spec: specs/renderer-left-inset/spec.md

## Goal
Add a `left_inset` to the renderer that mirrors `set_top_inset`, so the terminal grid narrows by a caller-set left band; must not violate specs/renderer-left-inset/spec.md or the project's dependency-direction / no-gui-crates rules.

## Scope & non-goals
- In: `crates/renderer/src/lib.rs`, `crates/renderer/src/cells.wgsl`, `crates/renderer/src/glyph.wgsl`.
- Out: `tab_bar.rs` (drawing into the band), any `crates/app/**` wiring (setting a non-zero inset). `top_inset` stays; do not remove/rename it.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| new left-inset unit tests pass | all pass, ≥4 new assertions | `cargo test -p renderer` (grid_dims col-reduction 120→100, ≥1-col floor, zero-inset invariance, set_left_inset clamp round-trip) |
| uniform block sizes | `BgUniforms==32`, `GlyphUniforms==16` | `uniform_blocks_are_16_byte_aligned` test |
| clippy warnings | `== 0` | `cargo clippy --workspace --all-targets -- -D warnings` |

> Project perf budgets (120fps / ≤8ms) are goals, not gates (docs G1) — N/A here.

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — is `left_inset` a line-for-line twin of the `top_inset` seam (field + `set_left_inset`/`left_inset()` + `FrameCtx` field + `grid_dims` param + `write_uniforms`), with no wgpu/winit/app leak into renderer?
- **Naming / readability** — `left_inset` reads clearly at every call site; shader `px.x + u.left_inset` parallels `px.y + u.top_inset`.
- **Spec alignment** — SD1 (band = left inset, cols shrink), D5–D8; zero-inset (`0.0`) output byte-unchanged (R4.1); `set_left_inset` writes uniforms only, no cache invalidation (D8).
- **Uniform layout** — `left_inset` placed last scalar (BgUniforms offset 20, GlyphUniforms offset 12) so WGSL mirrors the Rust `#[repr(C)]` byte-for-byte (D7 known risk).

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the renderer edits above, their unit tests, doc comments, in-scope refactors.

**🛑 STOP — call a human (escalate):**
- Any edit needed OUTSIDE the declared surface (`lib.rs`/`cells.wgsl`/`glyph.wgsl`).
- A change that would break the dependency direction (`common ← term-core ← renderer ← app`) or require a GUI crate.
- Uniform 16-byte alignment cannot be preserved (would need a struct-size change).
- insta snapshots change unexpectedly → run `/insta-review`, do NOT blind-accept.
- Anything conflicting with this Goal or the spec.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)
<!-- A fresh contract has none. -->
