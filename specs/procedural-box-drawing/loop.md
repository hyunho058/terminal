# loop.md — Loop Contract: procedural-box-drawing

## Goal
Verify that the completed implementation of `specs/procedural-box-drawing/spec.md`
satisfies its 11 requirements / 48 GWT sub-requirements — box-drawing and
block-element characters `U+2500..=U+259F` (except `╱╲╳`) drawn as cell-snapped
procedural quads instead of shaped glyphs, with non-intercepted output unchanged.

**This is a verification run: the implementation is already complete.** Iteration 1
does no work; it goes straight to VERIFY.

## Scope & non-goals
- In: `crates/renderer/**` (`box_draw.rs`, `atlas.rs`, `lib.rs`) and
  `specs/procedural-box-drawing/**`.
- Out: everything else. `term-core` must stay GPU/window-free
  (`dependency-direction`); no new dependencies (`no-gui-crates`); no shader or
  vertex-layout change — D6 exists precisely so none is needed.

---

## Gate 1 — Pass/Fail (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| no `unwrap`/`expect` in new production code | `! awk '/^#\[cfg\(test\)\]/{exit} /unwrap\(\)\|expect\(/{f=1} END{exit !f}' crates/renderer/src/box_draw.rs` |
| no new dependencies | `git diff --name-only -- '*/Cargo.toml' Cargo.toml \| grep -q . && exit 1 \|\| exit 0` |
| no shader/vertex-layout change | `git diff --name-only -- '*.wgsl' \| grep -q . && exit 1 \|\| exit 0` |
| `term-core` untouched | `git diff --name-only -- crates/term-core \| grep -q . && exit 1 \|\| exit 0` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| renderer tests passing | `≥ 189` | `cargo test -p renderer` |
| `box_draw` unit tests passing | `≥ 37` | `cargo test -p renderer box_draw` |
| codepoints classified procedural in `U+2500..=U+259F` | `== 157` (160 minus the 3 diagonals) | `classification_is_total_over_the_range` |
| diagonals left to the font | `== 3` (`U+2571..=U+2573`) | same test |
| quads for a plain `─` cell | `== 1` | `vertically_adjacent_box_stems_share_an_exact_edge` |

## Gate 3 — Qualitative (score /10 + grounds + action)

- **Spec alignment (R0–R10)** — every requirement has a test that actually
  exercises it; no GWT is satisfied only by assertion-free code.
- **Seam correctness under the arm-merge deviation** — opposing arms of equal
  weight are merged into ONE rect, which the spec did not describe. Verify this
  does not weaken R4.1 (no unpainted row at a junction), R0.1/R0.3 (adjacent cells
  share an exact edge), or the mixed-weight path (`╾`, `╿`).
- **Architecture fit** — `term-core` untouched, no new deps, no shader edit;
  interception lives at the documented point in `build_glyph_instances`, and
  `box_draw` stays a pure-geometry module testable without a GPU.
- **Test honesty** — the non-regression tests genuinely compare against a
  pre-interception reference path and cannot pass vacuously.

---

## Autonomy Boundary

**✅ Auto-fix:** anything within `crates/renderer/**` — geometry, tests, comments,
naming, refactors that keep every Gate-1 command green.

**🛑 STOP — escalate:**
- Editing any file outside `crates/renderer/**` (except this spec directory).
- Adding a dependency, editing a `.wgsl` shader, or changing the `GlyphInstance`
  vertex layout — each contradicts D6 and the spec's Constraints.
- Touching `crates/term-core/**`.
- Widening or narrowing D2's interception set, or reversing D2/D3/D5 — those were
  human decisions made against a documented alternative.
- Anything else conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop
in escalation.

---

## Lessons

### L1 — a regression test's reference must be the PRE-change algorithm
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: For every non-regression test, verify its reference path implements the
  *pre-change* algorithm and not a copy of the post-change one, and that the test
  asserts its own premise. A reference that reproduces the new logic makes the
  comparison tautological and it can never fail.
- note: `reference_instances` marked box chars `faces[i] = None`, exactly as the new
  code does, so `want` and `got` came from identical run structures — R9.1's claim
  (splitting a face-0 run moves nothing) went unexercised while the test showed green.

### L2 — a test comment may name only what the test can fail on
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Every test comment/name must describe only behaviour the test can actually
  fail on. Reject any comment naming characters, layers, or requirements the test
  structurally skips.
- note: Hit three times in one run — a connectivity sweep whose comment cited
  `╼╽╾╿` while `has_h && has_v` skipped all twelve single-axis chars; an R8.5 test
  that asserted `Atlas` behaviour while claiming to constrain sibling layers; an
  R8.6 test that duplicated an existing compile-time assert.

### L3 — sweep degenerate cell sizes, not just the reference cell
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `cargo test -p renderer every_procedural_character_emits_something_visible`
- note: Widening this sweep from `(17,34)` to `(1,1)..(64,128)` found three real
  bugs at once — `band` returning a negative start so a heavy stem painted into the
  row above, a stub collapsing to zero width, and fills rounding away to nothing.
  None were visible at the reference cell size.
