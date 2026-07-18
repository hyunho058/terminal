# loop.md — Loop Contract: scrollback

- pre-approved-unattended: yes

## Goal
Add a bounded scrollback history + viewport offset to `term-core` that works under tmux (alt-screen aware), per `specs/scrollback/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `crates/term-core/src/grid.rs`, `crates/term-core/src/snapshot.rs`, `crates/term-core/src/scrollback.rs`, `crates/term-core/src/parser.rs`, `crates/term-core/tests/scrollback.rs`
- Out: renderer, app, common, other crates. `term-core` stays GPU/UI-free (SD2).

---

## Gate 1 — Pass/Fail

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
| existing grid_snapshot tests | still pass (offset 0 == today) | `cargo test -p term-core --test grid_snapshot` |
| new scrollback tests | `≥ 4 pass` (scroll, eviction, alt-screen) | `cargo test -p term-core --test scrollback` |

## Gate 3 — Qualitative
- **Architecture fit** — `term-core` stays headless/GPU-free (SD2); scroll is display-only, never writes the PTY.
- **Spec alignment** — R0–R5 GWTs hold (retain history, offset auto-increment, eviction clamp, alt-screen frozen-primary behavior).
- **Naming / readability** — `view_offset` / `at_bottom` / `at_top` / alt-screen state read clearly.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the declared term-core files above.
**🛑 STOP — escalate (park):**
- Editing any file outside the declared surface (esp. renderer/app).
- Changing the `GridSnapshot` public shape in a way that breaks the renderer contract (SD5) after a consumer exists.
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.
