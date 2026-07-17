# loop.md — Loop Contract: settings

- pre-approved-unattended: yes

## Goal
Make font/theme/ligature settings live: a directory-watching config reloader that re-applies to the running renderer/tabs, per `specs/settings/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `crates/common/src/config.rs`, `crates/common/src/watch.rs`, `crates/app/src/settings.rs`
- Out: everything else. Registers via the SD4 seam. Consumes `Renderer::apply_config` from glyph-rendering (does not add it).

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
| common config/watch tests | `≥ 3 pass` (reload, invalid-kept, atomic-rename survival) | `cargo test -p common` |

## Gate 3 — Qualitative
- **Architecture fit** — `common` stays UI/GPU-free (SD2); watcher watches the parent dir (survives atomic-rename saves); app applies via the SD4 seam, not by editing `terminal_app.rs`.
- **Spec alignment** — R0–R3 GWTs hold (live reload, invalid-config-kept, atomic-rename survival, font-size re-fit, ligature/theme apply).
- **Naming / readability** — the `ConfigWatcher` emit contract (only `Ok`) is clear.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the declared surface above.
**🛑 STOP — escalate (park):**
- Editing files outside the declared surface (esp. renderer, term-core, app `terminal_app.rs`).
- If `Renderer::apply_config` does not exist yet from glyph-rendering — escalate rather than adding it to the renderer (out of surface).
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.
