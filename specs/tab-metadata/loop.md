# loop.md — Loop Contract: tab-metadata

> Supervisor contract for the tab-metadata feature. Batch-approved via /build-order.
- pre-approved-unattended: yes    # batch-approved at build-order Phase-2 gate (2026-07-20)
- spec: specs/tab-metadata/spec.md

## Goal
Provide per-tab git-branch and AI-agent state, refreshed **off the UI thread and cached by `TabId`**, so no render frame performs filesystem or process I/O (SD4), exactly as `specs/tab-metadata/spec.md` specifies. Expose a `TabId`-keyed read API + roster-update entry point for `sidebar-row-redesign` to drive; define its own app-side agent-identity enum (never reference `renderer::AgentSkin`, per foundations D13).

## Scope & non-goals
- In: `crates/app/src/tabmeta/**`, `crates/app/src/main.rs` (the single `mod tabmeta;` + pump registration the spec owns), `crates/ai-monitor/src/**`.
- Out: `crates/app/src/tabs.rs`, `terminal_app.rs`, `session.rs` (reimplement `proc_pidinfo` inside `tabmeta/` — `cwd_of_pid` is private and out of surface); `term-core`; the renderer; adding a `UserEvent` variant.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| tests | `cargo test --workspace` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --check` |
| typecheck | N/A — Rust; covered by `cargo build` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| clippy warnings | `= 0` | Gate-1 clippy |
| render-path I/O | `= 0` filesystem/process calls on any draw/`record`/`sync` path | code review of the frame path + the spec's cost checkpoints |
| branch/agent unit tests | present & green (git-HEAD parse incl. detached; agent match table; process-walk bound `MAX_CHILDREN`; change-only publish) | `cargo test --workspace` |
| new crate deps | `= 0` | `git diff Cargo.toml` empty (Cargo.lock hook-blocked) |
| `unwrap()` outside tests | `= 0` new | grep touched files |

## Gate 3 — Qualitative  (score + grounds + action)
- **Architecture fit** — worker thread + channel + caller-owned `wake` callback mirrors `LogWatcher::spawn`; `term-core` untouched; no renderer types referenced.
- **Naming / readability** — the `TabId`-keyed read API + roster-update entry point read clearly at the seam `sidebar-row-redesign` calls.
- **Spec alignment** — agent-state derivation (process presence + JSONL recency), the cadence (active vs background poll), pid-death mid-walk handling, and `cd`-without-process-change invalidation all match the spec.
- **SD4 compliance** — no per-frame syscall added; polling is off-thread and change-gated.

---

## Autonomy Boundary
**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs, behaviour-preserving refactors, the macOS `proc_*` syscall reimplementation inside `tabmeta/`.
**🛑 STOP — escalate:** adding a crate dependency; adding a `UserEvent` variant (out of surface — use the existing `OverlayPending`/callback path) or otherwise editing `terminal_app.rs`/`tabs.rs`/`session.rs`; any `#[cfg(target_os)]`; SSH/remote or non-zsh shell logic; anything conflicting with the Goal or spec. (Note the spec's own Known Gaps — native-vs-node `claude`, `KERN_PROCARGS2` entitlement — are boundary uncertainties: escalate if the chosen detection can't be made to work rather than guessing.)

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.

---

## Lessons
<!-- A fresh contract has none. -->
