# loop.md — Fix regression: drag-select + copy + paste not working at runtime

## Goal
After the drag-select-copy feature build, the user reports: drag-select/copy is not
working AND Cmd+V paste is not working either. All unit tests + build/clippy are
green, so this is a **runtime/integration** defect the pure-logic tests can't see.
Find the actual runtime root cause (not a guess), fix it, and confirm at runtime:
(1) dragging highlights text, (2) Cmd+C copies it, (3) Cmd+V pastes clipboard text.

Two of the three failing behaviors (Cmd+C copy, Cmd+V paste) are ⌘-key shortcuts —
a likely common cause is that ⌘-combos don't reach `on_key` on macOS (IME suppress
gate, winit delivery, or event routing). The drag highlight is mouse-driven — a
possibly separate path. Instrument first, read the evidence, then fix.

## Gate 1 — Pass/Fail (binary, 100%)
- `cargo build` succeeds
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace` green

## Gate 2 — Quantitative
- No pre-existing test regressions (workspace test count stays ≥ its current pass count).
- Any new regression-guarding unit test the root cause admits is added (≥1 if the
  cause is unit-testable; if the cause is purely macOS event delivery, state that no
  headless test can cover it — do not fake one).

## Gate 3 — Qualitative (judged, with grounds + action)
- **root-cause-identified** — the fix targets a *diagnosed* cause backed by runtime
  evidence (a log line / observed behavior), not a speculative change. Grounds must
  cite the evidence.
- **all-three-work** — at runtime: drag highlights, Cmd+C copies, Cmd+V pastes.
  Confirmed by the user or an observable log, not asserted.
- **no-collateral** — the fix doesn't break typing, tab chords, IME/Hangul
  composition, or the sidebar.

## Autonomy note
No STOP-list items. Instrumentation (tracing) is temporary; remove or gate it before
declaring done. Runtime confirmation requires a live run + the user's interaction
(mouse + ⌘ keys + real pasteboard can't be unit-tested).

## Lessons
### L1 — runtime paths need runtime verification
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: A feature whose gates are all green in unit tests can still be fully broken at
  runtime (macOS event delivery, IME gate, layer wiring). For input/render features,
  a live run with observed behavior is part of "done" — green units are necessary,
  not sufficient.
