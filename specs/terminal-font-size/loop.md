# loop.md — Loop Contract

> The supervisor document for `terminal-font-size` (`specs/terminal-font-size/spec.md`).
> Note from `agent-orchestrate` Phase 3.5: **Implementation (`T1`–`T4`) is already complete — start by verifying (`Phase 2 VERIFY`), do not redo the work unless a gate fails.**

## Goal
Verify that runtime terminal font size zoom (`Cmd+=` / `Cmd++`, `Cmd+-`, `Cmd+0`) satisfies every requirement (`R0`–`R5`), constraint (`C1`–`C8`), and task (`T1`–`T4`, `TT0`–`TT5`) in `specs/terminal-font-size/spec.md` without violating workspace architecture or code rules.

## Scope & non-goals
- In:
  - `crates/common/src/config.rs` (`T1`, `TT1` — `FONT_SIZE_MIN/MAX/DEFAULT/STEP`, `Config::normalize()` clamping, `save_font_size`)
  - `crates/renderer/src/lib.rs` (`T2`, `TT2` — `font_size_pt`, `font_size_override`, `set_font_size`, `clear_font_override`, `font_size_logical`, `apply_config` override preservation)
  - `crates/app/src/input.rs`, `crates/app/src/ime_chord.rs` (`T3`, `TT3` — `FontAction`, `font_action`, Korean 2-Set `chord_key`, generic `classify_delivery` / `remember_write` / `needs_persist`)
  - `crates/app/src/terminal_app.rs` (`T4`, `TT0`, `TT4`, `TT5` — `do_font_action`, `persist_font_size`, `last_written_font`, `apply_font_action`, `font_override_authoritative`, `should_clear_font_override`)
- Out:
  - Modifying `crates/term-core` (`C1`) or `crates/renderer/src/card_atlas.rs` (`D2` / Non-goals)
  - Per-tab font sizes, pinch-to-zoom, Cmd+scroll zoom, or toast/HUD overlay (Non-goals)

---

## Gate 1 — Pass/Fail  (100% required, binary)
Each line is a command; a non-zero exit fails the gate.

| Check | Command |
|-------|---------|
| build | `cargo build --workspace --all-targets` |
| typecheck | `cargo check --workspace --all-targets` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| term-core untouched (C1) | `test -z "$(git diff --name-only -- crates/term-core)"` |
| CardAtlases untouched (D2) | `test -z "$(git diff --name-only -- crates/renderer/src/sidebar_row.rs)"` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| Requirement test coverage (`R0.1`, `R1.1`–`R1.4`, `R2.1`–`R2.6`, `R3.1`–`R3.4`, `R4.1`–`R4.4`, `R5.1`–`R5.3`) | `19/19 (100%)` | Count requirements covered by passing unit tests in `common`, `renderer`, and `app` |
| Production `.unwrap()` count in modified crates (`C6`) | `0` | Inspection of `git diff` outside `#[cfg(test)]` modules across `crates/common`, `crates/renderer`, `crates/app` |
| Compiler / Clippy warnings | `0` | `cargo clippy --workspace --all-targets -- -D warnings` |

## Gate 3 — Qualitative  (score + grounds + action)
Score each /10 (threshold: `≥ 8/10` each, evaluated by a separate checker subagent per `maker ≠ checker`). A score alone is invalid — grounds and action are mandatory.

- **Architecture fit** — Does the change preserve crate boundaries (`common <- term-core <- renderer <- app`), reuse `Renderer` atlas rebuild + `settings::refit`, and follow the existing `written_sidebar` / `written_monitor` echo-suppression pattern?
- **Naming / readability** — Are `FontAction`, `font_action`, `set_font_size`, `clear_font_override`, `font_size_logical`, `save_font_size`, and `should_clear_font_override` clear and consistent at call sites?
- **User-flow naturalness** — Are `Cmd+=`/`Cmd++`/`Cmd+-`/`Cmd+0` handled at the right priority (blocked by modal `RenameOverlay`, dismissing open card menu, no-op at bounds preserving selection, updating IME cursor area, and surviving Korean 2-Set IME)?
- **Spec alignment** — Does the implementation match `D1`–`D11` and `R0`–`R5` in `specs/terminal-font-size/spec.md` without scope creep?

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests,
docs/comments, local renames, behaviour-preserving refactors.

**🛑 STOP — call a human (escalate):**
- DB schema changes
- Data-loss-capable migrations
- Auth / permission / access-control policy changes
- Payment / security-sensitive changes (secrets, crypto, billing)
- Anything conflicting with this contract's Goal or `specs/terminal-font-size/spec.md`

## Exit criteria
The loop exits only when **every** gate above passes and the Evidence Report is
emitted. Hitting a 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Each lesson is a mistake this loop already made, hardened so it can't recur.
> Phase 0 loads `status: active` lessons: `gate1-command` / `gate3-item` forms are
> folded into the gates above; `context` forms are read as guidance. Lessons are
> scoped to THIS `loop.md` — they do not propagate to other contracts. The human
> owns this section (curation); see SKILL.md "Self-Learning Lessons" + Phase 5.

<!-- Append entries below. A fresh contract has none. -->
