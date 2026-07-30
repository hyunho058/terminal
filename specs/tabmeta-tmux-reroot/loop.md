# loop.md — Loop Contract

> Supervisor document for `tabmeta-tmux-reroot` (feature 2 of the `tmux-pane-resolution`
> set). Approved in batch by `/build-order` Phase 2 on 2026-07-29.

- pre-approved-unattended: yes
- spec: specs/tabmeta-tmux-reroot/spec.md
- depends_on: tmux-root-resolution
- build_order: build_order.md

## Goal
Make the tab card follow the active tmux pane: replace `Deriver::tab`'s prologue with D1's
single published snippet so branch, agent badge and task label describe the pane's project —
without violating `specs/tabmeta-tmux-reroot/spec.md`.

## Scope & non-goals
- In: `crates/app/src/tabmeta/worker.rs` — the feature's **declared surface**, and nothing else.
- Out: `tmux.rs`, `proc.rs`, `agent.rs`, `mod.rs` (feature 1 owns them — **consume only**);
  `tabsession.rs` (feature 3); `tabs.rs`, `sidebar_model.rs` (no owner in this partition —
  the title/branch divergence is a recorded Known Gap, NOT this loop's to fix); `term-core`,
  `renderer`, `common`; `Cargo.toml`; `Cargo.lock`.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| typecheck | N/A — `cargo build` is the type check for Rust |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| warm non-tmux tab | `PROC_CWD == 1` and `TMUX_LIST == 0` | counter test, `probe::serial()` (R0.6, R5.2) |
| gate delta (agent off, k=0) | `PROC_EXE == 1`, `PROC_CHILDREN == 1` | counter test with `enabled_agent = false` (R5.2) |
| both toggles off | `PROC_CWD == 0`, `PROC_EXE == 0`, `PROC_CHILDREN == 0`, `TMUX_LIST == 0` | counter test (R3.1) — proves the read is gated **outside** the match |
| agent-only state | `PROC_CHILDREN == 2` (gate + `agent_of_pid`'s own walk) | counter test (R3.3) |
| construction purity | every counter `== 0` after `Deriver::new()` | `empty_roster_does_no_work` ordering (R6.1) |
| pre-existing suite | all 5 `worker.rs` tests pass **with original assertions**: `empty_roster_does_no_work`, `steady_state_repo_derivation_does_no_git_read`, `disabling_git_clears_branch_and_stops_git_work`, `dead_pid_derives_to_blank_without_panic`, `publish_bumps_generation_only_on_change` | `cargo test --workspace` (R5.1, R3.4, R4.3, R2.3) |
| published shape | `TabMeta` has exactly `branch`, `agent`, `task` | inspection (R4.2) |
| new dependencies | `0` | `git diff crates/app/Cargo.toml` is empty |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — does it consume feature 1's API without reaching into its files, and
  leave the cadence, the differ and the roster untouched?
- **Prologue fidelity (D1)** — is D1's published snippet transcribed faithfully: `enabled`
  bound first, `root` bound **before** the guard block, and the cwd read gated **outside** the
  `match` (a `match (enabled, cwd_of_pid(root))` evaluates the scrutinee eagerly and fires the
  syscall with both toggles off — this is the single most likely implementation error)?
- **Naming / readability** — is the rebinding of `root` on the fallback path legible at `:82`?
- **Spec alignment** — D2 (roster untouched), D3 (memo on `Deriver`), D4 (all three open toggle
  states re-root), D7 (`TabMeta` shape and differ unchanged), D9 (pane-death fallback and the
  rebound `root` redirecting `agent_of_pid` too).

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs/comments,
local renames, behaviour-preserving refactors — all **inside `worker.rs`**.

**🛑 STOP — call a human (escalate):**
- **Any edit outside `crates/app/src/tabmeta/worker.rs`** — a partition violation. In
  particular: if feature 1's API turns out to need a change, escalate; do NOT edit `tmux.rs`.
- Adding or changing a dependency in any `Cargo.toml`; touching `Cargo.lock`.
- Any `#[cfg(target_os = …)]` branch.
- `unwrap()` / `expect()` / `panic!` outside `#[cfg(test)]`.
- **Relaxing, weakening, or deleting any of the 5 pre-existing `worker.rs` assertions** to
  reach green — the spec's Constraints forbid it explicitly.
- Changing `TICK`, `BG_INTERVAL`, `should_refresh`, `publish_if_changed`, or `TabMeta`'s shape.
- Anything conflicting with this contract's Goal or the spec.

## Exit criteria
Exits only when **every** gate passes and the Evidence Report is emitted. Hitting a 🛑 boundary
ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — check the surface before editing, not after
- cause: escalation
- source: auto-unattended
- form: gate3-item
- status: active
- check: "Surface check: run `git status --porcelain crates/` and confirm EVERY modified path is in this contract's Scope-In list. A fix that is correct on its merits is still a partition violation if it lands in a sibling's file — and a flaky test discovered here may well live there."
- note: iteration 2 edited `tabmeta/tmux.rs` (feature 1's committed surface) while chasing a flaky test in this feature's loop; the Gate-3 checker caught it and the human resolved it by amending feature 1 separately.

### L2 — a counter assertion keyed to a live process count is flaky, not wrong
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: "Environment-dependent assertions: does any counter assertion depend on a LIVE quantity (a child count, a pid list) sampled outside the counted window? Other tests in the same binary spawn processes without holding `probe::serial()`, so such a value can change under you. Assert the invariant part; prove the exact shape against a fake."
- note: `PROC_EXE == 1 + k` with k = children_of(me).len() failed ~5% of full-crate runs; the fix asserts `>= 1` here and proves `1 + k` deterministically in tmux.rs against a fake tree.
