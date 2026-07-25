# loop.md — Loop Contract: selection-input

- feature-id: selection-input
- spec: specs/selection-input/spec.md
- pre-approved-unattended: yes   # approved at build-order batch gate 2026-07-24

## Goal
Implement the app-side selection input per `specs/selection-input/spec.md`: a pure
`pixel_to_cell` helper, a left-press/drag/release selection lifecycle (the one sanctioned
event-loop exception), and two consumer-facing `AppCtx` fields (`bracketed_paste`,
`selection_text`) — without regressing existing sidebar/IME/wheel input.

## Scope & non-goals
- In: `crates/app/src/{terminal_app.rs, seam.rs, input.rs}` (the declared surface).
- Out: term-core/renderer code; clipboard read/write; the highlight layer; Cmd+C/Cmd+V handlers (siblings).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — `cargo build` is the typecheck in Rust |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| pure helper tests (R1 pixel_to_cell, R4 AppCtx) | `≥ 1, all green` | `cargo test -p app` |
| test coverage % | N/A — no coverage tooling configured | — |
| fps / latency | N/A as a gate — project GOALS not gates (docs/domain G1) | — |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — Cmd+C/Cmd+V stay in the seam; ONLY the pointer pipeline (MouseInput Pressed/Released + CursorMoved drag) touches the event loop (SD4); `AppCtx::new`'s existing signature + 5 test callsites unchanged (builder methods).
- **Naming / readability** — `pixel_to_cell` reads like the sibling `sidebar_press` helpers.
- **Spec alignment** — `moved` keyed on the focus cell leaving the anchor (jitter-safe); arm only when `pixel_to_cell` is `Some` (dead-band no-op); `selection_text` gated on `selection().is_some()` (no per-keystroke grid clone); plain click clears (R1–R5).
- **No regression** — sidebar Card/NewTab/Collapse clicks, hover, wheel scroll, and IME composition behave exactly as before (R0.1, R5.1).

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs/comments,
behaviour-preserving refactors — all WITHIN the declared surface.

**🛑 STOP — call a human (escalate):**
- Adding a bespoke Cmd+C/Cmd+V event-loop branch instead of using the seam (SD4 violation).
- Adding any GUI/widget crate; importing `winit` into a crate that must stay window-agnostic.
- Editing `Cargo.lock` (PreToolUse hook blocks it).
- Adding a `#[cfg(target_os = ...)]` branch (macos-only rule).
- Editing any file OUTSIDE the declared surface, or changing `AppCtx::new`'s signature (breaks sibling callers).
- Any regression to existing sidebar/IME/wheel behavior, or anything conflicting with the spec's Decisions/SDs.

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons
<!-- A fresh contract has none. -->

### L1 — a match arm whose body is a lone `if` trips clippy::collapsible_match
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- note: A match arm `Pat => { if cond { .. } }` (body is only the `if`) fails `clippy::collapsible_match` under `-D warnings`. Fold the condition into the arm as a guard: `Pat if cond => { .. }` (e.g. the `MouseInput { Released, Left, .. } if inner.dragging =>` arm).
