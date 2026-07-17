# loop.md — Loop Contract: tabs

- pre-approved-unattended: yes

## Goal
Turn the single-session app into tabbed zsh sessions with a tab bar, and expose the SD4 event-loop registration seam, per `specs/tabs/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `crates/app/**`, `crates/renderer/src/tab_bar.rs`
- Out: term-core, common, other renderer files, other crates.

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
| app builds & launches | `app` binary runs, logs "terminal started" | `cargo run -p app` (headless smoke via logs) |

## Gate 3 — Qualitative
- **Architecture fit** — the tab bar is a registered `DrawLayer` (SD3); the SD4 seam (InputHandler/Overlay registries) is real and app-state-free on the renderer side (SD2).
- **Spec alignment** — R0–R6 GWTs hold (independent sessions, session lifecycle, tab bar model, input routing + seam, resize propagation to background tabs).
- **Naming / readability** — the SD4 seam (`AppCtx`, `Handled`, registration) reads clearly — siblings depend on it.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within `crates/app/**` and `crates/renderer/src/tab_bar.rs`.
**🛑 STOP — escalate (park):**
- Editing files outside the declared surface (esp. renderer core, term-core).
- Changing the SD4 seam contract in a way that breaks a sibling (image-paste/ai-monitor/settings) after it consumes it.
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.
