# loop.md — Loop Contract

> Supervisor document for `ai-monitor-context-window`. Approved interactively on
> 2026-09-12, as the Phase 3.5 verification gate of a `/agent-orchestrate` Sequential
> Pipeline run that had already executed T1–T5.

- spec: specs/ai-monitor-context-window/spec.md
- posture: **verification of a completed implementation** — iteration 1's WORK step is a
  no-op, so VERIFY is effectively the first action. Do not redo the work.

## Goal
Prove that the shipped change makes the ai-monitor CONTEXT tile's denominator a value
READ from a session's `/context` output rather than one guessed from the model id — and
that it does so without disturbing the 37-of-42 sessions that never ran `/context`.

## Scope & non-goals
- **In** (the feature's declared surface): `crates/ai-monitor/src/telemetry.rs`,
  `crates/ai-monitor/tests/telemetry.rs`,
  `crates/ai-monitor/tests/fixtures/context_command.jsonl`,
  `crates/app/src/dashboard.rs`, `docs/architecture.md`.
- **Out**: everything else — `term-core`, `renderer`, `common`, every other `app` module,
  `crates/ai-monitor/tests/fixtures/session.jsonl` (asserted whole by an existing
  snapshot; D13 exists precisely so it is not touched), any `Cargo.toml`, `Cargo.lock`.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| typecheck | N/A — `cargo build` is the type check for Rust |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| format | `cargo fmt --all --check` |

## Gate 2 — Quantitative  (measured vs threshold)

Every threshold below is a number the spec itself states, or a constraint it names. None
is invented here.

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| R0.1 the headline | `context_pct == 31` at `context_used` 217,737 against a 700,000 reading | `a_real_context_log_makes_the_tile_agree_with_the_command`, folding the fixture |
| R0.1 control | the SAME snapshot with the reading removed yields `22` | same test — the two numbers of the defect side by side |
| R0.1 tile text | `tile_of(model(...), "CONTEXT").value == "31%"` | same test |
| R0.2 preservation | `session.jsonl` folds to `context_window_reading == None`, and `context_pct` equals the table→`promoted_window` result recomputed independently | `a_log_that_never_ran_the_command_is_byte_identical_to_before` |
| R0.3 Antigravity | `context_pct == None` and the tile reads `—` when `context_used == 0`, reading present | `an_antigravity_style_session_never_reaches_the_reading` |
| R6.3 gate discipline | both counters `== 0` across the fixture's 4 decoy records (`/context` echo, prose quoting both anchors, `/model` output with `1M`, a non-`local_command` system record) | `the_context_fixture_reads_nothing_from_its_four_decoys` |
| mutation survivors | **0 absolute** over the change's mutants (`cargo mutants -j 4 --in-diff <branch diff>`) — an absolute count, never a ratio, per CLAUDE.md | cargo-mutants `missed.txt` empty AND `timeout.txt` empty |
| fixture encoding | raw `ESC` (0x1b) bytes in `context_command.jsonl` `== 0` | `the_context_fixture_carries_escaped_not_literal_escapes` |
| new dependencies | `0` | `git diff -- '*Cargo.toml' Cargo.lock` is empty |
| `unwrap()` outside tests | `0` in both changed source files | scan above the `#[cfg(test)]` line |
| Non-goal survivors | `promoted_window`, `MODEL_WINDOWS` and `env.context_window` all still present and reachable | grep + the R0.2/R4.1 tests that exercise each path |
| numerator untouched | `context_used`'s formula (`input + cache_read + cache_creation`) has no diff line | `git diff` on `crates/ai-monitor/src/telemetry.rs` |

## Gate 3 — Qualitative  (score + grounds + action; checker-scored)

- **Dependency direction** — does `ai-monitor` stay free of internal crate deps, and does
  `app` remain the only consumer? (`.claude/rules/dependency-direction.md`.)
- **D6 fidelity — the reading is TERMINAL** — is `promoted_window` genuinely not consulted
  on the reading path? This is the decision the spec reversed mid-review: promoting a
  700,000 reading yields 1,000,000, a denominator `/context` never prints, at exactly the
  occupancy where the tile matters most.
- **D9's four-arm predicate** — is it a PREDICATE evaluated at consumption (so a model that
  returns revives the reading) rather than a mutation? Is the fourth arm
  (`Some(a)` → `None`) written with its cost stated, given the spec flags it as in tension
  with D4?
- **D1a — one gate, used twice** — do the read path and both counters hang on the SAME
  condition, checked on raw content before `strip_sgr`?
- **Non-goals held** — no category breakdown drawn, no separate autocompact-buffer model,
  no change to `context_used`, nothing removed from the fallback cascade, and the panel
  never runs `/context` itself.
- **Comment quality** — do the new comments record *why* a decision went the way it did
  (especially the reversals), in the register the surrounding file already uses?

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/format/type errors, missing tests for code already
in scope, docs and comments, local renames, behaviour-preserving refactors — all **inside
the Scope-In list**.

**🛑 STOP — call a human (escalate):**
- **Any edit outside the Scope-In list** — in particular `session.jsonl`, whose whole-file
  snapshot D13 exists to protect.
- Adding or changing a dependency in any `Cargo.toml`; touching `Cargo.lock`.
- Any `#[cfg(target_os = …)]` branch (`.claude/rules/macos-only.md`).
- `unwrap()` / `expect()` / `panic!` outside `#[cfg(test)]`.
- **Weakening, relaxing or deleting an existing assertion to reach green** — including
  adding a `cargo mutants` `exclude_re` entry instead of fixing what the survivor reveals.
- Anything that conflicts with the approved spec: its Decisions, its Non-goals, or its
  Constraints.

## Exit criteria
Exits only when **every** gate passes and the Evidence Report is emitted. Hitting a 🛑
boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — verify a clause's attribution by deleting it, not by reading it
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "Attribution check: for every comment claiming that clause X is what refuses input Y, delete X and re-run Y. If Y is still refused, the comment is crediting the wrong clause — and the guard X is probably untested. Then check the converse: find the input that X alone refuses, and pin it."
- note: `header_denominator`'s comment credited the numerator check with refusing `.claude/agents 534 tokens (…)`; that input dies one statement earlier at the denominator parse. The claim was wrong in the original and wrong again after being narrowed without measuring. Deleting the numerator check left all 1562 tests green, and cargo-mutants could not have asked for the missing test either — it only replaces `header_denominator`'s whole body, which other tests already kill. `the_numerator_check_buys_one_case_and_not_the_others` now pins both halves.
