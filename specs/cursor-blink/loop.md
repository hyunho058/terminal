# loop.md — Loop Contract: cursor-blink

> Interactive /loop (human-approved). Add a blinking cursor to the terminal.

## Goal
Draw a blinking block cursor at the grid cursor position. Implemented as a
renderer `DrawLayer` so the blink runs off the existing continuous-redraw loop and
is NOT frozen by the idle-CPU snapshot cache (layers record every frame regardless
of a grid cache hit/miss). No `term-core` change; no change to the idle-cpu cache.

## Scope & non-goals
- In: `crates/renderer/src/cursor_layer.rs` (new — a `DrawLayer` + its solid-rect
  pipeline + ~530ms `Instant` blink), `crates/renderer/src/lib.rs` (module decl +
  a `CURSOR_Z` z-order const), `crates/app/src/terminal_app.rs` (register the layer).
- Out: `term-core` (cursor position already in `GridSnapshot`), the idle-cpu cache
  logic, DECTCEM `?25` cursor hide/show, and cursor-shape (`CSI q`) — noted follow-ups.

---

## Gate 1 — Pass/Fail
| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — `cargo build` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --check` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative
| Metric | Threshold | How measured |
|--------|-----------|--------------|
| blink phase is a pure, tested fn | on for ~530ms, off for ~530ms | unit test on `cursor_visible(elapsed)` at t=0, 300ms, 600ms, 900ms |
| cursor positioned at the grid cursor | quad rect = cursor cell (col·cw, row·ch + top_inset) | headless test: layer emits one quad at `snapshot.cursor` when visible, zero when blinked-off |

## Gate 3 — Qualitative
- **Architecture fit** — cursor is a `DrawLayer` (records every frame), not in the cached grid instance path, so it blinks despite the idle-cpu cache; no `term-core` edit; z-order above glyphs, below the tab bar.
- **Idle-cpu intact** — the continuous-redraw loop + occluded pause are unchanged; the cursor layer piggybacks on the frames idle-cpu already presents; idle-cpu's own tests stay green.
- **Behavior** — a visible block cursor at the shell prompt that blinks ~530ms; the character under it stays legible (blended, not opaque).
- **Naming / readability** — `cursor_visible`, `CursorLayer`, `CURSOR_Z` clear.

---

## Autonomy Boundary
**✅ Auto-fix:** the cursor layer, its pipeline/shader, registration, tests, lint/fmt.
**🛑 STOP — escalate:**
- Any change to the idle-cpu cache logic (`draw_to` snapshot equality) or to
  `term-core`'s `GridSnapshot` (the layer approach exists to avoid both).
- Anything conflicting with this contract's Goal.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop
in escalation, not completion.

## Lessons
<!-- fresh contract; none -->
