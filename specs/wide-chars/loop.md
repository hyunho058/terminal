# loop.md — Loop Contract: wide-chars (double-width character support)

> Interactive /loop (human-approved). Fix: typing double-width characters (Korean
> 한글 / CJK / emoji) then pressing backspace erases part of the shell prompt.

## Goal
Track and render double-width characters correctly so the grid's column model
matches the shell's. Root cause (confirmed by feeding real zsh output through the
grid): `print()` advances `cursor.col += 1` for EVERY char, but 한글/CJK/emoji are
2 columns wide. zsh advances by 2 and its backspace erases 2 columns per char, so
our cursor runs N columns left of zsh's and the erases overshoot into the prompt's
right end (reproduced: `PROMPT> ` → `PRO`).

## Scope & non-goals
- In:
  - `crates/term-core/Cargo.toml` — add the `unicode-width` crate.
  - `crates/term-core/src/grid.rs` — `print()` advances by the char's display width
    (1/2); a width-2 char writes a leader cell + a continuation sentinel (`'\0'`,
    carrying the leader's colors) in the next cell; wrap a wide char that would
    straddle the right margin; drop zero-width chars (first cut).
  - `crates/term-core/src/snapshot.rs` — `to_text()` skips continuation cells.
  - `crates/renderer/src/lib.rs` — `build_glyph_instances` skips continuation cells
    (the wide leader's glyph already spans 2 cells; no overlapping glyph in the
    spacer). Column-based positioning means wide chars land 2 cells apart.
- Out: full grapheme clustering / ZWJ emoji sequences / combining-mark stacking
  (per-char `unicode-width` only); wide-char-aware selection; backspace-at-col-0
  line-wrap (separate deferred item).

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
| wide char advances 2 cols | `한글` → cursor col 4 (not 2) | unit test on the grid |
| continuation + wrap | leader+`'\0'` written; a wide char at the last col wraps | unit tests asserting cells + cursor |
| prompt survives Korean backspace | the real captured 한글 type+backspace leaves the prompt intact | regression test feeding the captured/synthetic double-backspace sequence; assert prompt row unchanged |
| copy skips spacer | `to_text()` of a wide-char row has no `'\0'`/stray space | unit test |

## Gate 3 — Qualitative
- **Correctness** — cursor column model matches a real terminal (verified against zsh output); wide char never splits across the right margin; zero-width handled safely.
- **Architecture fit** — width logic in `term-core` (GPU-free); the renderer only skips the continuation sentinel; `Cell` PartialEq still covers the sentinel so idle-cpu's cache stays correct (SD2 from idle-cpu).
- **Bug fixed** — the Korean type+backspace repro no longer eats the prompt; 한글 renders without overlapping glyphs.
- **Naming / readability** — the continuation sentinel + width handling are clearly documented.

---

## Autonomy Boundary
**✅ Auto-fix:** the width/print logic, the sentinel, the renderer skip, `to_text`, the `unicode-width` dep (Cargo.toml only — cargo manages Cargo.lock), tests, lint/fmt.
**🛑 STOP — escalate:** a change to the idle-cpu cache logic or a `Cell` structural
change beyond what's needed; anything conflicting with this contract's Goal.

## Exit criteria
Every gate passes and the Evidence Report is emitted.

## Lessons
<!-- fresh; none -->
