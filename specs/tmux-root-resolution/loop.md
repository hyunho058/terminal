# loop.md — Loop Contract

> Supervisor document for `tmux-root-resolution` (feature 1 of the `tmux-pane-resolution`
> set). Approved in batch by `/build-order` Phase 2 on 2026-07-29.

- pre-approved-unattended: yes
- spec: specs/tmux-root-resolution/spec.md
- build_order: build_order.md

## Goal
Ship `crates/app/src/tabmeta/tmux.rs` — the mechanism that answers "which pid does this tab's
metadata derivation start from" — returning the tmux client's `pane_pid` when the tab's tree
contains an attached client and the tab's own shell pid otherwise, without violating
`specs/tmux-root-resolution/spec.md`.

## Scope & non-goals
- In: `crates/app/src/tabmeta/tmux.rs` (new), `crates/app/src/tabmeta/proc.rs`,
  `crates/app/src/tabmeta/agent.rs`, `crates/app/src/tabmeta/mod.rs` — the feature's
  **declared surface**, and nothing else.
- Out: `worker.rs` and `tabsession.rs` (sibling features own them); `tabs.rs`,
  `sidebar_model.rs`, `overlay.rs` (no feature owns them in this partition); `term-core`,
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
| non-tmux spawn cost | `TMUX_LIST == 0` | counter test, holding `probe::serial()` (R0.2) |
| tmux spawn cadence | `TMUX_LIST <= 1` per tick, **including the failure path** | counter test with a failing fetch (R5.4, R7.2) |
| tty stat cost | `TTY_STAT == N` (attached clients), **not** M×N | counter test across M tabs in one tick (R7.3) |
| tab-side tty read | `PROC_TTY == 1` per `tty_dev_of_pid` call, success or `None` | counter test (R1.5) |
| gate syscall cost | `PROC_EXE == 1 + k`, `PROC_CHILDREN == 1` | counter test, `k` = direct children (R2.6) |
| headless testability | every R2–R5 test passes **with no tmux binary and no tmux server** | run the suite; no test may require live tmux (R8.1) |
| pre-existing suite | `empty_roster_does_no_work`, `steady_state_repo_derivation_does_no_git_read` pass **unrelaxed** | `cargo test --workspace` (R7.4) |
| new dependencies | `0` | `git diff crates/app/Cargo.toml` is empty (R8.2) |

## Gate 3 — Qualitative  (score + grounds + action)
Score each /10. A score alone is invalid — grounds and action are mandatory.

- **Architecture fit** — does it honour `dependency-direction` (app-crate-local; `term-core`
  learns nothing) and match `proc.rs`'s existing wrapper idiom (absent-on-failure, counter
  bumped as the first statement)?
- **Purity split (SD5/D12)** — is `parse_clients` genuinely pure, is the `stat` the *only*
  injected seam in `resolve_rdevs`, and is `select_pane` pure integer comparison?
- **Naming / readability** — are `tmux_client_of`, `effective_root`, `effective_root_uncached`,
  `rdev_of_path` clear at the call site?
- **Spec alignment** — D1 (`Option<u32>`), D2 (named `NODEV`), D5 (root-then-depth-1, exact
  case-insensitive basename), D6 (spawn `TmuxClient.exe`, never `PATH`), D7 (single entry,
  wall-clock guard, negative caching), D13 (liveness gate), D14 (default socket only),
  R8.3 (`pub(crate) mod tmux;` — the sibling feature cannot compile without it).

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs/comments,
local renames, behaviour-preserving refactors — all **inside the declared surface**.

**🛑 STOP — call a human (escalate):**
- **Any edit outside this feature's declared-surface globs** — a partition violation.
- Adding or changing a dependency in any `Cargo.toml`; touching `Cargo.lock`.
- Any `#[cfg(target_os = …)]` branch (`macos-only` rule).
- `term-core` / `renderer` gaining a dependency on this code (`dependency-direction`).
- `unwrap()` / `expect()` / `panic!` outside `#[cfg(test)]`.
- **Relaxing, weakening, or deleting a pre-existing test assertion** to reach green.
- Any GUI/widget crate (`no-gui-crates`).
- Anything conflicting with this contract's Goal or the spec.

## Exit criteria
Exits only when **every** gate passes and the Evidence Report is emitted. Hitting a 🛑 boundary
ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — a counter assertion must not derive its expectation from itself
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: "Self-referential assertions: does any Gate-2/counter assertion compute its expected value FROM the measured value (e.g. `let k = COUNTER.get() - 1; assert COUNTER.get() == 1 + k`)? Such an assertion is a tautology that holds for every input. Every threshold must come from an independent source measured outside the counted window."
- note: iteration 1 shipped `assert PROC_EXE == 1 + (PROC_EXE - 1)`, which proved only `PROC_EXE >= 1`; the Gate-3 checker caught it and the fix measures k via `children_of(pid).len()` before `reset_all()`.

### L2 — an orchestration function with no injected seam cannot have its branches measured
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: "Unreachable branches: for each function that composes several impure calls, can every branch be exercised by a test on this machine? If the first gate always closes (e.g. no tmux is running), the later branches are unreachable and any 'measurement' of them is structural, not counted. Require an injected-seam variant (`*_with`) mirroring `agent::walk` / `classify_with`."
- note: `effective_root` initially hardcoded all five of its syscalls, so D13's liveness gate, the negative cache and the M-tabs-one-fetch budget were untestable; extracting `effective_root_with` made all three measurable and each is now proven by mutation.

### L3 — prove a new assertion can fail before trusting it
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: after adding or repairing a gate assertion, apply a deliberate mutation to the code it guards and confirm the test FAILS; two of this run's assertions passed only because they could not fail.
