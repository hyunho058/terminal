# loop.md — Loop Contract: fix-cursor-left-inset

> Human-approved (interactive). Fix the cursor (and pasted image) horizontal
> position after the left tab sidebar landed.

## Goal
The overlay `DrawLayer`s that position content by grid cell must shift x right by
`ctx.left_inset` — exactly as the bg/glyph shaders already do (`px.x + left_inset`)
— so the blinking cursor sits at the real cursor cell (right after the prompt),
not `left_inset` px to its left. `image_layer` has the identical omission.

## Scope & non-goals
- In: `crates/renderer/src/cursor_layer.rs`, `crates/renderer/src/image_layer.rs`.
- Out: `overlay_layer.rs` (ai-monitor floating panel — a full-window overlay, separate concern), `term-core`, `crates/app`, the shaders (already correct).

---

## Gate 1 — Pass/Fail
| Check | Command |
|-------|---------|
| build | `cargo build` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative
| Metric | Threshold | How measured |
|--------|-----------|--------------|
| cursor block_rect x includes left_inset | test asserts `x == col*cw + left_inset` | `cargo test -p renderer cursor` |
| all renderer tests pass | 0 failures | `cargo test -p renderer` |
| clippy warnings | `== 0` | `cargo clippy --workspace --all-targets -- -D warnings` |

## Gate 3 — Qualitative
- **Correctness** — cursor block and image tiles land at `left_inset + col*cw` (matching the glyph shader), verified by a unit test with a non-zero inset.
- **Architecture fit** — the layers read `ctx.left_inset` from `FrameCtx` (already present); mirrors the existing `ctx.top_inset` handling; no term-core/app change.
- **Consistency** — both cursor and image layers fixed the same way; no other grid-cell-positioned layer left unshifted.

---

## Autonomy Boundary
**✅ Auto-fix:** the x-offset in the two layers + their unit tests.
**🛑 STOP — escalate:** any change needing term-core / app / shader edits; anything conflicting with this Goal.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted.

---

## Lessons
<!-- fresh contract -->
