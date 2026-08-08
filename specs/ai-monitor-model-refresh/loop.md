# loop.md — Loop Contract: ai-monitor-model-refresh

## Goal
The ai-monitor panel must show the model the session is **currently** running, not the first
one it ever saw. Reproduced from a real log (`64c00dbb`): the session ran `claude-fable-5`,
the user ran `/model` to switch to `claude-opus-5`, five later assistant turns logged
`claude-opus-5` — and the panel still read `claude-fable-5`.

Root cause: `crates/ai-monitor/src/telemetry.rs:309` guards the assignment with
`if self.model.is_none()` (first-write-wins), copied in shape from the `session_id` fold
directly above, where first-wins is correct because a session id never changes.

## Scope & non-goals
- **In:** `crates/ai-monitor/src/telemetry.rs` (the fold) and its tests; `crates/app/src/dashboard.rs`
  only if the display side needs it.
- **Also in (user chose to fix together):** the context-window tier. `context_window()` maps
  `claude-opus-*` to 200k, but a measured session reached **514,930** tokens of single-turn
  context — 257% of 200k, which the panel clamps to a useless `>100%`. Verified: the log
  carries no window marker of any kind (every `[1m]` hit in a real log is prose, not data), so
  the window cannot be read — but it CAN be deduced: a session that used 514,930 tokens is
  provably not on a 200k window. Promote to the smallest tier that fits the observed usage.
- **Out:** `crates/term-core/**`.

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| term-core untouched | `sh specs/ai-monitor-right-panel/check-no-term-core.sh` |
| no stray subagent artifacts (L4) | `test -z "$(git status --porcelain --untracked-files=all -- crates/ \| grep -v '^ M')"` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| suites green | `== 18` | count of `test result: ok` |
| the real-log repro | model resolves to `claude-opus-5` | a test folding the fable-5 → opus-5 sequence from `64c00dbb` |
| `<synthetic>` never displayed | 0 occurrences | a test folding a trailing `<synthetic>` line |
| pre-existing assertions **weakened or deleted** | `== 0` | diff test bodies; an assert count that RISES is fine |

> **Metric corrected mid-run.** This row first read "pre-existing assertions changed == 0",
> which conflated an *assertion* with a *fixture input*. Two existing tests used
> `context_used = 256_551` as their ">100%" case; under the promotion rule that value is
> evidence of a 1M window and correctly reads 26%, so the fixture had to move
> (256_551 -> 1_256_551) with every `assert_eq!` left byte-identical. Leaving it would have
> pinned promotion OFF and silently re-frozen the bug. The bar that actually matters is
> weakened-or-deleted, not changed.
| the 1M repro | `context_used = 514_930` on `claude-opus-5` reports ~51%, not `>100%` | a test |
| a 200k session is unaffected | still reports its true % against 200k | a test |

## Gate 3 — Qualitative (checker subagent — maker ≠ checker)

- **Correctness of the rule** — is "last real model wins" right? Consider: `<synthetic>`;
  a `message.model` on a non-assistant record; whether a subagent's model can reach this file
  (the codebase says nested agents write to a `subagents/` sibling that is never read — verify);
  and whether the very first fold still populates the field.
- **`session_id` untouched** — first-wins is CORRECT there. A fix that "consistently" flips
  both is a regression.
- **Backfill vs tail agreement** — `open()` folds the file from scratch, then the watcher tails
  appends. Both paths must converge on the same model, including when the switch straddles the
  backfill/tail boundary.
- **Window promotion is a deduction, not a guess** — it must only ever fire on evidence
  (observed usage exceeding the assumed window), never on a model-name hunch. A session that
  stays under 200k must be completely unaffected. The `>100%` clamp must survive for the case
  where usage exceeds even the largest known tier — that state is still possible and still
  means "the window is wrong".
- **The config override still outranks everything** (`[ai_monitor] context_window`).

---

## Autonomy Boundary
**✅ Auto-fix:** the fold rule, its tests, doc comments, and a display-side change if needed.
**🛑 STOP — escalate:**
- Any edit under `crates/term-core/**`.
- Changing `session_id`'s first-wins semantics.
- Weakening or deleting an existing assertion to make a gate pass.
- ~~Widening into the context-window/1M problem~~ — **struck**: the user explicitly chose to
  fix it alongside, and Scope records that. Left visible rather than deleted so the contract
  shows the boundary moved by decision, not by drift.

## Exit criteria
Every gate passes and the Evidence Report is emitted.

## Lessons
_(carried in spirit from specs/ai-monitor-right-panel/loop.md: never `git checkout --` an
uncommitted tree; check `git status` for subagent leftovers before trusting a gate.)_
