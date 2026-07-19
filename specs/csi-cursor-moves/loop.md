# loop.md — Loop Contract: csi-cursor-moves

> Interactive /loop (human-approved). Fix: editing a line (esp. when it wraps)
> corrupts / erases the shell prompt.

## Goal
Implement the missing relative CSI cursor-movement sequences so line edits that
reposition the cursor no longer desync the grid and overwrite the prompt. Root
cause (confirmed by feeding real zsh output through the grid): `csi_dispatch`
handles only `H/f`, `J`, `K`, `m` — it silently drops `A`/`B`/`C`/`D`/`G`/`d`,
which zsh emits to move the cursor across wrapped rows during an edit.

## Scope & non-goals
- In: `crates/term-core/src/parser.rs` — add CSI arms:
  - `A` CUU (cursor up N), `B` CUD (down N), `C` CUF (forward N), `D` CUB (back N)
  - `G` CHA (cursor to column N, 1-based), `d` VPA (cursor to row N, 1-based)
  - default/`0` param = 1; clamp to the grid; GPU-free (term-core only).
- Out: renderer / app; backspace-at-col-0 line-wrap (a separate follow-up — zsh
  uses CSI moves here, not BS-wrap); scroll regions (`r`), insert/delete-char
  (`@`/`P`), mouse. Note as follow-ups if relevant.

---

## Gate 1 — Pass/Fail
| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — `cargo build` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --check` |
| tests | `cargo test --workspace` (incl. `insta` grid snapshots) |

## Gate 2 — Quantitative
| Metric | Threshold | How measured |
|--------|-----------|--------------|
| each CSI move covered | A/B/C/D/G/d each have a passing unit test | move the cursor from a known pos with default and explicit N; assert `grid.cursor()`; verify clamping at edges and `0`→1 default |
| prompt-preservation regression | prompt row survives a wrapped edit | a synthetic "print prompt+text, CUU/CUF reposition, EL erase" sequence asserts the prompt row is intact and the right region cleared |

## Gate 3 — Qualitative
- **Correctness** — 1-based vs 0-based conversion for `G`/`d` (like `H`); `0` param treated as 1 for relative moves; saturating arithmetic; clamped to grid bounds.
- **Architecture fit** — cursor moves live in `term-core` (GPU-free, headless-testable); reuse `grid.cursor()` + `grid.move_to()`; no renderer/app change.
- **Bug fixed** — the wrapped-edit repro no longer leaves stale glyphs / erased prompt.
- **Naming / readability** — CSI arms clearly commented (CUU/CUD/CUF/CUB/CHA/VPA).

---

## Autonomy Boundary
**✅ Auto-fix:** the CSI arms, any small grid cursor helper, tests, lint/fmt.
**🛑 STOP — escalate:** a change touching the renderer/app or the idle-cpu/cursor
layers; anything conflicting with this contract's Goal.

## Exit criteria
Every gate passes and the Evidence Report is emitted.

## Lessons
<!-- fresh; none -->
