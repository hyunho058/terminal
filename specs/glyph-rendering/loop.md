# loop.md — Loop Contract: glyph-rendering

- pre-approved-unattended: yes

## Goal
Replace the block-font renderer with real `swash`-atlas glyph rendering and establish the SD3 `DrawLayer` seam, per `specs/glyph-rendering/spec.md` — without violating that spec or the mac-terminal-v1 Shared Decisions.

## Scope & non-goals
- In: `crates/renderer/**`
- Out: everything else (term-core, app, common, other crates). The renderer must not depend on winit (SD2).

---

## Gate 1 — Pass/Fail (100% required, binary)

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
| renderer unit tests | `≥ 3 pass` (atlas, cluster, metrics) | `cargo test -p renderer` |

## Gate 3 — Qualitative (score /10 + grounds + action)
- **Architecture fit** — `renderer` stays winit-free (SD2); the glyph pass is registered as a `DrawLayer` (SD3), not hard-coded.
- **Spec alignment** — R0–R6 GWTs hold (real glyphs, atlas cache, ligature clusters, baseline placement, `.notdef`, z-ordered layers).
- **Naming / readability** — the `DrawLayer`/`FrameCtx` API reads clearly at the call site (it is the seam siblings consume).

---

## Autonomy Boundary
**✅ Auto-fix:** anything within `crates/renderer/**` — shader, atlas, pipeline, tests, refactors.
**🛑 STOP — escalate (park):**
- Editing any file **outside `crates/renderer/**`**.
- Changing the `DrawLayer` / `FrameCtx` seam in a way that breaks its documented provisional contract *after* a sibling consumes it.
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.
