# loop.md — Loop Contract: real-bold-faces

> Post-execution **verification** gate for an implementation that is already complete
> (T1–T8, Sequential Pipeline, landed on `fix/bold-hangul-weight` over baseline `ae4be97`).
> Iteration 1's WORK step is a deliberate no-op so VERIFY is effectively the first action.
> Code changes only to fix defects that verification actually surfaces.
- spec: `specs/real-bold-faces/spec.md`

## Goal

Prove the landed implementation satisfies all 9 requirements / 31 GWT sub-requirements of
`specs/real-bold-faces/spec.md` — bold text is drawn from the font file's **real** bold face,
synthesis survives only as a fallback, and **no Hangul syllable loses ink when bolded** — without
violating the spec's `## Constraints` or the subtle correctness rules in D1–D18.

A green build is **not** accepted as proof on its own, and neither is a green test suite: the
defect this spec fixes (593 of 11,172 syllables eroded by `embolden`) was invisible to a suite of
1,460 passing tests, because every renderer observable reported a *decision* and none counted ink.

## Scope & non-goals

- **In:** verification of `crates/renderer/src/{font_set,glyph,atlas,lib,sidebar_row,tab_bar}.rs`,
  `crates/renderer/tests/{bold_faces,golden_frame}.rs`, `docs/architecture.md`; and auto-fix of
  defects those checks surface (missing tests, lint, docs, local corrections).
- **Out:** re-doing completed work; new features beyond the spec; the spec's declared Non-goals
  (new font files, a font-config surface, `term-core` changes, re-designing the shaping algorithm,
  variable-font `wght` interpolation, re-tuning `BOLD_EMBOLDEN_FRAC`); and the spec's
  `## Known Gaps` G-A/G-B/G-C, which are recorded by decision rather than fixed.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| system-font tests | `cargo test -p renderer -- --ignored` |

> **Why the `--ignored` run is scoped to `-p renderer` and not the workspace.** It is a *required*
> Gate-1 member — R0.2, R1.1, R1.2, R1.5, R7.1 and R7.2 are only provable against the real
> `AppleSDGothicNeo.ttc` / `Menlo.ttc`, and the project parks those behind
> `#[ignore = "depends on a system font path"]`. But the workspace-wide `--ignored` run also picks
> up `crates/app`'s `presence_fd::r0_1_live_agents_are_told_apart_and_resolve_to_different_logs`,
> which greps `/bin/ps` for a live `agy` process and fails when none is running. That is an
> environmental precondition in a different crate that reads the process table, and it is
> **pre-existing** — it cannot be affected by a renderer change. Scoping the gate keeps it
> meaningful instead of permanently red for a reason unrelated to this spec. VERIFY must
> nonetheless *confirm* that attribution rather than assume it (see Gate 3).

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| workspace tests failed | `== 0` | `cargo test --workspace` |
| renderer system-font (`--ignored`) tests failed | `== 0` | `cargo test -p renderer -- --ignored` |
| clippy warnings | `== 0` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `cargo fmt --all --check` diff | empty | exit status |
| **Hangul syllables losing ink when bolded** | `== 0 / 11172` | `bold_faces::no_hangul_syllable_loses_ink_when_bolded`, ppem 14 **and** 28 |
| **Hangul syllables below the 1.15× floor** | `== 0 / 11172` | `bold_faces::every_hangul_syllable_reads_as_bold` |
| ASCII below the floor outside the recorded set | `== 0` | `bold_faces::the_latin_bold_exceptions_have_not_grown` |
| bold cells carrying less ink than regular, **in captured pixels** | `== 0` | `golden_frame::bold_cells_carry_more_ink_than_regular_ones` |
| mutation survivors **in code this change introduced** | `== 0` | `cargo mutants -f …/{font_set,atlas}.rs`, survivors attributed by enclosing fn |
| `.unwrap()`/`.expect()` outside tests **added** by this change | `== 0` | per-file pre-`#[cfg(test)]` count vs `git show ae4be97:<f>` |
| `#[cfg(target_os` occurrences in `crates/` | `== 0` | `grep -rn 'cfg(target_os' crates/ \| wc -l` |
| `term-core` non-comment refs to wgpu/winit | `== 0` | `grep -rn 'wgpu\|winit' crates/term-core/src crates/term-core/Cargo.toml` |
| new `unsafe` blocks without a `// SAFETY:` comment | `== 0` | `grep -rn unsafe crates/renderer/src/*.rs` |
| sub-requirements traced to a test or code site | `== 31 / 31` | maker-run trace of `## Requirements` R0–R8 to `file:line` |
| 55 MB TTC mapped more than once per `FontSet::load` | `== 0` | `font_set::styled_faces_share_the_regular_face_mapping` (`Arc::ptr_eq`) |

## Gate 3 — Qualitative  (score + grounds + action — scored by a SEPARATE checker subagent)

Targets the rules that adversarial review caught during spec derivation, plus the two the
implementation itself surfaced. None of these is established by a passing suite; each needs a read
of the actual code.

- **D15 / R3.3–R3.4 — the key names the face the id came from.** Every construction site of
  `GlyphKey` must set `slot` to the face `id` was actually taken from. A single site that hardcodes
  `StyleSlot::Regular` while shaping from a styled face reintroduces the `Ǟ`/`ǟ` collision silently.
- **D7 / R3.1–R3.2 — face selection is style-blind.** No path may let a style attribute change
  *which chain entry* a character comes from; a coverage hole must fall back to that entry's
  regular with synthesis, never to the next entry and never to nothing.
- **D2 / R2.1–R2.2 — absent ≠ broken.** A style the file does not ship must be silent (or the log
  fires every startup for AppleSDGothicNeo's missing italic); a face that exists but fails to parse
  must warn. Check the two really are distinguished, not conflated.
- **D9 / R2.5 — axes resolve independently.** Bold-italic Hangul must keep the *real* w700 face and
  synthesize only the slant. Falling back to full synthesis would restore the erosion for that one
  style.
- **D16 / R4.3 — the accepted advance divergence.** 612 Gothic codepoints differ in advance between
  regular and bold (86 of them actually reachable). Confirm `center_offset`/`cell_span` still place
  correctly and that cell *width* is untouched.
- **Spec alignment (D1–D18).** Did any decision get quietly dropped or contradicted during
  implementation? In particular D8's tie-break fields/tolerance and D13/D18's "no re-tune without
  citing the floor".
- **Test honesty.** The `bold_faces.rs` suite was *vacuous* on first write (a missing bold slot made
  it skip, reporting `4 passed` in 0.00s). Confirm the current helpers fail rather than skip when
  fonts are present but a slot is missing, and that the golden-frame additions measure ink
  *quantity* rather than re-testing the existing presence predicate.

---

## Lessons

### L1 — a font test that skips is a font test that lies
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: **Skip vs fail.** Every test whose precondition is a font, a face or a GPU must distinguish *absent* from *present but wrong*. Absent (no system font, no adapter, no such chain entry) may skip. Present-but-wrong — the family loaded and its bold slot is empty — must **fail**, because that is the regression the test exists to catch. Check each new or changed test for an early `return`/`continue` that could make it assert nothing, and for a counter proving it compared at least one thing.
- note: `bold_faces.rs` reported `4 passed; 0 failed` in 0.00s with the bold probe deliberately disabled — every assertion skipped. Fixed, and then the *same* mistake reappeared one iteration later in `ligatures_shape_identically_from_a_styled_face`, written to close a traceability gap. Twice in one change is a pattern, not an accident.

### L2 — falsify a guard before trusting it
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: Before claiming a new test protects a behaviour, apply the defect it targets and confirm it goes red. Three separate guards in this change passed while vacuous until falsified this way — the T6 sweeps, the T7 pixel tests, and the ligature test. Falsification also classifies a surviving mutant honestly: applying `metric_key -> None` showed it to be genuinely equivalent on the fonts available, which no amount of reading the code would have settled.
