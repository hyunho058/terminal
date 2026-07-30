# loop.md — Loop Contract

> Supervisor document for `tabsession-tmux-reroot` (feature 3 of the `tmux-pane-resolution`
> set). Approved in batch by `/build-order` Phase 2 on 2026-07-29.

- pre-approved-unattended: yes
- spec: specs/tabsession-tmux-reroot/spec.md
- depends_on: tmux-root-resolution
- build_order: build_order.md

## Goal
Make the ai-monitor overlay's session hint see a tmux tab's agent: extract the pure, injectable
`choose_claude` and re-root `hint_for`, so a tmux tab stops returning an empty hint — **without
ever** returning a hint that would be confidently applied to the wrong directory. Must not
violate `specs/tabsession-tmux-reroot/spec.md`.

## Scope & non-goals
- In: `crates/app/src/tabsession.rs` — the feature's **declared surface**, and nothing else.
- Out: `tmux.rs`, `proc.rs`, `agent.rs`, `mod.rs` (feature 1 owns them — **consume only**);
  `worker.rs` (feature 2); `overlay.rs`, `session.rs`, `tabs.rs`, `terminal_app.rs`, `seam.rs`
  (no owner in this partition — the projects-directory limit is a recorded Known Gap, **NOT**
  this loop's to fix); `crates/ai-monitor`; `term-core`; `Cargo.toml`; `Cargo.lock`.

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
| core purity | `choose_claude` performs **0** direct syscalls — every one arrives via an injected closure | inspection + table tests with hand-built `ProcTable` (R1.1) |
| walk-count guard | counting `find` closure reads **exactly 1** when `root == shell_pid` | table test (R1.4) |
| gate cost when unrooted | counting `cwd_of` closure reads **0** when `root == shell_pid` | table test (R4.3) |
| cwd-gate semantics | admits **only** on `(Some(a), Some(b))` with `a == b`; `None` on either side (incl. both) falls through | table tests (R4.1, R4.4, R4.5, R4.6) |
| budget bound | `resolved_root_with` returns `shell_pid` in roughly `RESOLVE_BUDGET` when the injected resolver sleeps past it; returns the resolver's value when prompt | table test with injected resolver (**R5.1–R5.2**; R5.3 is a restatement of R5.1 under load and is not separately tested — row corrected rather than left overclaiming) |
| walk bounds | `MAX_DEPTH == 6`, `MAX_VISITED == 512` unchanged | inspection (R2.1) |
| headless testability | every R1–R5 test passes with **no tmux, no live tty, no live pid** | run the suite (D9) |
| pre-existing suite | all 7 `tabsession.rs` tests pass **with original assertions** | `cargo test --workspace` (R2.4) |
| new dependencies | `0` | `git diff crates/app/Cargo.toml` is empty |

## Gate 3 — Qualitative  (score + grounds + action)

- **Architecture fit** — does it consume feature 1's `pub(crate)` API via
  `crate::tabmeta::tmux::` (NOT `super::tmux::` — `tabsession` is a crate-root module) without
  editing it, and preserve the module's read-only-observation contract?
- **Control-flow fidelity (D2)** — is D2's published interleaved flow transcribed faithfully:
  the cwd gate **inside** the `if root != shell_pid` branch (so a tab-provenance hit is never
  gated), and every non-admitting path **falling through** rather than returning `None`?
- **The regression this feature exists to prevent (D7)** — does the gate compare the found
  `claude`'s cwd (**not** the pane shell's), so a cross-directory pane can never reach
  `resolve_session`'s one-sided `since_ms` branch and produce a sticky `owned: true`?
- **Naming / readability** — `choose_claude`, `resolved_root`, `resolved_root_with`.
- **Spec alignment** — D1 (returns `Option<u32>`, not a built `SessionHint`), D3 (bounds
  unchanged, per-call `MAX_VISITED`), D5 (both fields read off the chosen pid), D8 (bounded
  wait with `shell_pid` fallback, in-surface).

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs/comments,
local renames, behaviour-preserving refactors — all **inside `tabsession.rs`**.

**🛑 STOP — call a human (escalate):**
- **Any edit outside `crates/app/src/tabsession.rs`** — a partition violation. In particular:
  if feature 1's `tmux` module is not `pub(crate)` and the call will not resolve, **escalate** —
  do NOT edit `tabmeta/mod.rs` (that is feature 1's R8.3 obligation).
- Editing `overlay.rs` to re-root the projects directory — the recorded Known Gap; a human owns it.
- Adding or changing a dependency in any `Cargo.toml`; touching `Cargo.lock`.
- Any `#[cfg(target_os = …)]` branch.
- `unwrap()` / `expect()` / `panic!` outside `#[cfg(test)]`.
- **Relaxing, weakening, or deleting any of the 7 pre-existing `tabsession.rs` assertions.**
- Changing `SessionHint`'s shape, `resume_id`, `start_ms`, `looks_like_session_id`, or the
  `ProcTable` parsing contract.
- Anything conflicting with this contract's Goal or the spec.

## Exit criteria
Exits only when **every** gate passes and the Evidence Report is emitted. Hitting a 🛑 boundary
ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — a one-candidate fixture cannot test a preference or a fallback
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: "Fixture blind spot: for any rule of the form 'prefer A over B' or 'if A is rejected, fall back to B', is there a fixture where A and B BOTH exist? With only one candidate present, 'A wins', 'B is the fallback' and 'there was only one' are indistinguishable, and both a reordering and a `return None` survive."
- note: every cwd-gate test used a table whose tab shell had no claude, so mutants that reordered the branches or returned None instead of falling through both survived; a BOTH_SAMPLE table with a claude in each subtree kills both.

### L2 — do not let a Gate-2 row claim a requirement that has no test
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: the budget row cited R5.1-R5.3 while only R5.1 and R5.2 had tests; the row was corrected rather than left overclaiming, since an unbacked citation reads as coverage on review.
