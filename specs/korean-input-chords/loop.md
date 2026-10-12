# loop.md — Loop Contract

> Supervisor document for verifying `specs/korean-input-chords/spec.md` (approved 2026-10-11).

## Goal
The uncommitted implementation on `feat/korean-input-chords` meets every automatable
requirement of `specs/korean-input-chords/spec.md` (R0.1, R0.3, R1, R2, R3, R5, R6 by test or
inspection; R0.2 and R4 by code inspection) without violating its Decisions or Constraints.

## Scope & non-goals
- In: `crates/app/src/{input,ime_chord,terminal_app,copy,clipboard,overlay,rename,capture,lib}.rs`,
  `docs/architecture.md` (G9), `specs/hangul-ime-and-font-fallback/spec.md` (note).
- Out: R7.3 — the 11-step manual checklist (D9), which needs a human typing under 2-Set Korean;
  synthetic events bypass the input method. Reported as pending for the user.
- Out: CI's newer clippy (1.98 vs local 1.95) — cannot run locally.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| typecheck | N/A — covered by `cargo build` / clippy (Rust) |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| mutation survivors, `chord_key` | `= 0` | `cargo mutants -f crates/app/src/input.rs -F chord_key -j 4` |
| mutation survivors, `ime_chord.rs` | `= 0` | `cargo mutants -f crates/app/src/ime_chord.rs -j 4` |
| R1.1–R1.6, R2.2–R2.3, R3.1–R3.8, R5.1–R5.3 sub-reqs with a named `#[test]` | `= 100%` (19 of 19) | map each sub-req to a test by name/comment (`grep -n "R1\.\|R2\.\|R3\.\|R5\." crates/app/src`) |

## Gate 3 — Qualitative  (score + grounds + action; checker-scored)

- **Spec alignment of the commit tail** — replay after BOTH `place_commit` exits; `on_ime`
  propagates exit; duplicate guard before `suppress_key`; `replayed_ts` reset on every non-empty
  commit (D7, D8, R4, R5.3).
- **Architecture fit** — thin read-only AppKit shim, pure decisions, dependency direction, no
  new crates (Constraints).
- **Readability / comment truthfulness** — new comments state facts that match the measured
  Research.
- **Docs accuracy** — G9 and the Hangul-spec note match the code and the spec (R6).

---

## Autonomy Boundary

**✅ Auto-fix:** lint/type/format, missing in-scope tests, docs/comments, local renames,
behaviour-preserving refactors.

**🛑 STOP — call a human:** anything conflicting with the spec's Decisions/Constraints; the
usual DB/auth/payment/security list.

## Exit criteria
Every gate above passes and the Evidence Report is emitted.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Append entries below. A fresh contract has none. -->

### L1 — New Rust lines stay within 100 columns
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `! git diff -U0 -- '*.rs' | grep '^+' | grep -v '^+++' | awk 'length > 101' | grep -q .`
- note: rustfmt does not rewrap comments; scripted (non-Edit) comment edits left 154- and 133-column lines that only the Gate-3 checker caught.
