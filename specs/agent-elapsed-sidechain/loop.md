# loop.md — Loop Contract: agent-elapsed-sidechain

## Goal
Verify that the already-implemented change satisfies `specs/agent-elapsed-sidechain/spec.md`
R0–R10 and violates none of its Constraints: an async `Agent` launch must no longer be folded
as a completion, and a background agent must end at the time its `<task-notification>` reports
— 24m21s reading as `24m21s`, never `0s`.

**The implementation is complete.** This loop starts at VERIFY; iteration 1 does no work.

## Scope & non-goals
- **In:** `crates/ai-monitor/src/telemetry.rs`, `crates/ai-monitor/tests/telemetry.rs`,
  `crates/app/src/dashboard.rs` — the three files the change touched.
- **Out:** `crates/renderer/**` and `crates/term-core/**` (R9.2/R9.3); any second watch target,
  second accumulator, or recursion into `subagents/`; and `spec.md` itself — the spec is the bar
  being measured against, not an artifact this loop may bend to fit the code.

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| headless — no `~/.claude` (R10.1) | `H=$(mktemp -d) && HOME="$H" cargo test -p ai-monitor` |
| no non-occurring fixture shape (R10.2) | `! grep -rq '"isAsync"[[:space:]]*:[[:space:]]*false' crates/` |
| confinement (R9.2/R9.3) | `! git diff --name-only \| grep -qE 'crates/(renderer\|term-core)/'` |
| no `unwrap()` in the changed fold | `! grep -q 'unwrap()' crates/ai-monitor/src/telemetry.rs` |
| no `unwrap()` in the changed render | `! sed -n '1,/#\[cfg(test)\]/p' crates/app/src/dashboard.rs \| grep -q 'unwrap()'` |

> Every command above was run before this contract was proposed and is currently green.
> None is a placeholder.

> **Amended 2026-08-19, after iteration 2, with the human's approval.** Two Gate-2 rows
> below were unreachable **as written** — not because the implementation fell short, but
> because the metrics were mis-specified when this contract was drafted. The coverage row
> demanded that every sub-requirement be named by a *test*, when three of them (R9.2,
> R9.3, R10.4) are properties of the diff and the toolchain that only a Gate-1 command can
> assert; and the literal-site row pinned a count that a legitimately-added test helper
> changed. Both rows now measure the thing they were meant to measure. **No verification
> was weakened** — the underlying facts are still checked, by a command that runs every
> iteration. See `## Lessons` L4 for why this is recorded rather than quietly edited.

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| sub-requirement coverage | `44 / 44` — every `#### Rn.m` in spec.md is covered by at least one test **or Gate-1 command** | audit R-tags in the two test files; R9.2, R9.3 and R10.4 are diff/toolchain properties and are covered by the Gate-1 rows above, not by any test |
| **the defect's number** | folded `end_ms - start_ms == 1_461_000` ms **and** the rendered card value is exactly `24m21s` | `a_notification_ends_the_agent_it_names` + `a_background_agent_reports_its_real_duration_end_to_end` |
| **witness independence (R10.3)** | disabling either witness alone fails `>= 1` test, and the two failure sets are **disjoint** | temporarily neutralise one witness in `is_async_launch`, run `cargo test -p ai-monitor`, revert with `git checkout --`; repeat for the other |
| changed files | `== 3` | `git diff --name-only \| wc -l` |
| `Subagent` literals stating both new fields | **all of them** — 5 today; the count is informational, never a threshold | the build. The type has no `Default` derive, so a missed literal is a compile error whatever the count; pinning a number instead only breaks when a test legitimately adds a literal |
| fold assertions | `>= 78` in `crates/ai-monitor/tests/telemetry.rs` (was 51 before this feature) | `cargo test -p ai-monitor` |

## Gate 3 — Qualitative

- **Spec alignment (R0–R10)** — the 44 GWTs hold as *written*, not as convenient. A test that
  asserts a weaker thing than its sub-requirement states is a gate failure, not a pass.
- **The join key is the agent, not the call** — `on_task_notification` matches `<task-id>`
  against `agent_id`; nothing anywhere reads the notification's `<tool-use-id>`. Keying on the
  waking call was measured to complete 50 agents where the agent's own id completes 80.
- **Witness scoping is load-bearing, not incidental** — both witnesses are consulted only inside
  the `ToolKind::Agent` arm. A `Bash` result echoing the prose (this project's own research
  greps did exactly that) must be unable to reach the classifier.
- **The failure direction stays conservative (D2)** — an unrecognised result still completes the
  agent. No path may leave an agent permanently un-completable when the upstream format drifts;
  degrading to today's defect is the accepted outcome, degrading to "nothing ever completes" is
  not.
- **The contract widened minimally** — two fields on `Subagent`, nothing on `Card`, no new watch
  target, and `tally_call` still the single point a call is counted.
- **Comment honesty against a decaying corpus** — `~/.claude/projects/**` is a rolling ~30-day
  window, so absolute counts age out. No comment or doc-string may assert a number as a live
  fact when it is an as-of measurement, and none may claim a count that the current corpus
  contradicts.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** anything within the three in-scope files — lint/format/type
fixes, added or strengthened tests for behaviour already in scope, doc-comment corrections,
local renames, behaviour-preserving refactors that keep every Gate-1 command green.

**🛑 STOP — call a human (escalate):**
- Touching `crates/renderer/**` or `crates/term-core/**`. In particular, adding a field to
  `Card` to colour a value — the failure colour rides the existing `accent` channel by decision
  (D13), and widening `Card` is a renderer-contract change this feature refused.
- Adding a second watch target, a second accumulator, or recursion into `subagents/`.
- Moving, duplicating, or bypassing `tally_call`.
- Changing `SUBAGENTS_CAP` or the ring's "latest 32" contract.
- Adding a fixture containing `"isAsync": false` — a shape that occurs in no observed log.
- **Editing `spec.md` so the code passes.** If a requirement is genuinely wrong, that is an
  escalation with the evidence, not a quiet edit.
- Applying `.rs` edits through the shell instead of Write/Edit — it bypasses the `rustfmt`
  PostToolUse hook and surfaces much later as a `cargo fmt --check` failure.
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in
escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Recorded 2026-08-19 after the first run of this contract (2 iterations).
     `source: human-approved` — the human selected all four at Phase 5. -->

### L1 — code that forbids a literal must not contain that literal
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `! grep -rq '"isAsync"[[:space:]]*:[[:space:]]*false' crates/`
- note: iteration 1 added a test to enforce R10.2 and broke R10.2 doing it — the test's own comment quoted the forbidden string, so the guarding grep matched. Assemble such literals at runtime (`format!("{}isAsync{}", '"', '"')`) and never spell them in source or comments.

### L2 — establish WHY a test passes, not just that it does
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: For every test added or strengthened, name the mutation it claims to catch and confirm that mutation actually fails it. A test that passes because its input is rejected upstream of the behaviour under test is not covering that behaviour.
- note: the R3.3 test fed an EMPTY `<task-id>`, which `parse_task_notification` rejects before the join runs — so the assertion held no matter what the join did, and the comment's claimed guard (against an `unwrap_or_default()` join) would have passed too. Found by the Gate-3 checker, not by the suite. The same discipline retro-actively justified the two witnesses (disjoint failure sets) and the `end_ms` guard (exactly one test catches its removal).

### L3 — never pin a corpus count in a comment
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: No comment or doc-string may state a count measured from `~/.claude/projects/**` as a live fact. Use count-free wording, or a magnitude with an explicit as-of date.
- note: `cleanupPeriodDays` defaults to 30, so the log corpus is a rolling window that sheds old sessions — counts decay rather than grow. Two comments asserted numbers the corpus already contradicted (`killed (3)` → 4; `12.3%` → ~13.6%) within days of being written. Structural claims (`isAsync` is never `false`; agentId is bijective) reproduce across corpora; frequencies do not.

### L4 — run a metric before you make it a threshold
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: two Gate-2 rows in this contract were unreachable the moment they were written — a `44/44` coverage target that three structural sub-requirements can never satisfy by test, and a `== 4` literal count that any new test helper invalidates. Both looked reasonable while drafting and neither had been executed. Drafting a threshold and measuring it are different acts; do the second before committing to the first.
