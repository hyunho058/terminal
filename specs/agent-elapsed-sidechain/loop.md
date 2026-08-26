# loop.md — Loop Contract: agent-elapsed-sidechain

## Goal
Verify that the already-implemented change satisfies `specs/agent-elapsed-sidechain/spec.md`
R0–R10 and violates none of its Constraints: an async `Agent` launch must no longer be folded
as a completion, and a background agent must end at the time its `<task-notification>` reports
— 24m21s reading as `24m21s`, never `0s`.

**Run 2 (2026-08-23) — four review findings.** The implementation shipped; a review then raised
four defects in the fold. All four were measured against the log corpus before any decision, and
all four turned out to be **unobserved** there — robustness, not live breakage. The human ruled on
each (see `## Run 2 decisions`): fix two, record-and-keep two. Two of the rulings authorise an
edit to `spec.md`, which the previous run's Scope forbade; that reversal is deliberate and
recorded below rather than taken silently.

## Run 2 decisions
| # | Finding | Ruling | Touches |
|---|---------|--------|---------|
| 1 | A sync `Agent` failure (`is_error: true`) renders in the success colour: `ok` is computed at `telemetry.rs:1166` and used by `tally_call`, but never reaches `ended_ok` on the non-launch path — so `failed_tool_calls` counts a failure the card denies | **fix + amend spec** | `telemetry.rs`, tests, `spec.md` (R7.1, R7.5, D13) |
| 2 | `restarted()` drops the `subagents` ring, and this feature widened the loss window from ~83ms to an agent's whole run | **keep; record the reasoning** | `spec.md` (known gap) |
| 3 | The prose witness matches on `starts_with`, so a genuine sync answer opening with that sentence would be misread as a launch receipt and strand the agent as a permanent LIVE ghost | **keep; record the measurement** | `spec.md` (D1 note) |
| 4 | An absent `<status>` tag is read as a failure — `None == Some("completed")` is `false`, so "could not read it" renders as Rose | **fix** | `telemetry.rs`, tests, `spec.md` (R7.3 GWT) |

Findings 2 and 3 are kept because **fixing them points the wrong way down D2**: preserving live
agents across a log truncation manufactures the permanent LIVE ghost D2 exists to refuse, and
narrowing the prose literal would discard the 8 real launches it was measured to be the only
witness for. Both are recorded with their grounds, not left silent.

## Run 3 decisions (2026-08-24)
A third review round re-reported findings 1 and 4 of run 2 — those rounds ran against
**pre-fix code**, and both are already fixed in `88dc62a` (`:1251`, `:1328`). What is new:

| # | Finding | Ruling |
|---|---------|--------|
| `is_async_launch` | The prose witness fires even when `toolUseResult` is present and says otherwise, so a genuine sync answer opening with the receipt sentence becomes a permanent LIVE ghost | **fix + amend D1.** Gate the prose witness on `toolUseResult` being ABSENT |
| `apply_claude` bare-string arm | A notification that FAILS to parse falls through to `scan_command_name`, registering a phantom skill from a `<command-name>` quoted in its body — the exact thing the comment two lines above forbids | **fix + add R4.4a** |
| `ToolKind::Agent` arm | `a.agent_id = agent_id` is unconditional, so a second result for the same `tool_use_id` carrying no `agentId` overwrites a good id with `None` and severs the join | **fix** (`is_some()` guard) |
| `result_text` | Clones a whole sync report — tens of KB — to test a 33-byte prefix, on every `Agent` result | **fix** (behaviour-preserving, early-terminating comparison) |
| `agent_elapsed` / `refresh_model` | A LIVE card's timer only advances when the log wakes the panel, so a quiet session freezes a 25-minute run at `12s` | **record only; split to its own spec.** Out of scope here |
| `Subagent` 2-bool lifecycle → enum; shared JSONL fixture module | **deferred**, both |

**Why the prose-witness ruling reversed.** Run 2 ruled *keep* on this witness, and that ruling
stands as made — it was taken on two proposals that both lost real detections (narrowing the
literal, dropping the witness). This round proposed a different shape, and the measurement
separates cleanly: of the results the prose witness alone catches, **all** have no
`toolUseResult` at all; of genuine sync answers, **all** carry one. Gating on its absence keeps
every detection and closes the false-positive path structurally.

**And what that first ruling missed.** The trade was named at approval time — "prose no longer
rescues an `isAsync` rename while `toolUseResult` still exists" — but its **size** was not
measured, and the direction alone made it sound partial. The Gate-3 checker measured it: **63 of
63** launch receipts also open with the prose, so the OR form had been carrying every one of them
under that drift and the ordered form carried none. A second ruling in the same run added the
`status` backstop (`status == "async_launched"`, measured 63 of 63 launches and 0 of 113
synchronous answers) so the drift is covered again without reopening the prose false positive.
Final shape: three readings, two domains, per-drift coverage stated in R10.3 — see the amendment
list under Scope.

## Scope & non-goals
- **In:** `crates/ai-monitor/src/telemetry.rs`, `crates/ai-monitor/tests/telemetry.rs`,
  `crates/app/src/dashboard.rs` — the three files the change touched — **and, for run 2 only,
  `specs/agent-elapsed-sidechain/spec.md`**, under the human ruling recorded above.
- **Out:** `crates/renderer/**` and `crates/term-core/**` (R9.2/R9.3); `crates/ai-monitor/src/lib.rs`
  (the `restarted()` caller — finding 2 is recorded, not fixed); any second watch target, second
  accumulator, or recursion into `subagents/`.
- **Run 3's approved amendments, as they finally stood.** Four, not the two first authorised —
  the extra two were approved mid-run after the Gate-3 checker surfaced what the first two broke:
  1. **D1** — the prose-witness gate (approved up front).
  2. **R4.4a** — the parse-failure path (approved up front).
  3. **R10.3** — amended because amendment 1 made its "the other still protects the panel" clause
     false, and it had been left standing. Approved once measured (63 of 63 launches lost under
     an `isAsync` rename).
  4. **R1.2a/R1.2b/R1.2c** — the sub-requirements that pin amendment 1's behaviour, plus the
     `status` backstop that repairs the coverage it cost. R1.4/R1.5 qualified to match.

  Also repaired, and **not** an amendment: the `### R4:` heading, deleted by accident when R3.3a
  was inserted and restored to byte-identical with `HEAD`. `crates/app/src/overlay.rs` and the
  event loop stay **out** — the frozen-timer finding is recorded, not fixed.
- **The spec-edit carve-out is narrow and one-directional.** Run 1 barred `spec.md` so the code
  could not be made to pass by moving the bar. That bar still stands for everything the run
  measures. What is permitted here is the opposite act: R7.1/R7.5/D13 assert that `ended_ok` is
  written *only* by the notification branch, and the human ruled that assertion **wrong** — so the
  spec is being corrected to what it should have said, with the code following. Any amendment that
  is not one of the four rulings above is still a 🛑 escalation.

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
| no spec section silently lost (L8) | `! comm -23 <(git show HEAD:specs/agent-elapsed-sidechain/spec.md \| grep '^#\{2,3\} ' \| sort -u) <(grep '^#\{2,3\} ' specs/agent-elapsed-sidechain/spec.md \| sort -u) \| grep -q .` |

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
| sub-requirement coverage | **all of them** — every `#### Rn.m` in spec.md is covered by at least one test **or Gate-1 command**; 48 as of run 3, and the count is informational | audit R-tags in the two test files; R9.2, R9.3 and R10.4 are diff/toolchain properties and are covered by the Gate-1 rows above, not by any test. The count is no longer a threshold: it was written `44 / 44`, went stale the moment a run added a sub-requirement, and was still reading `44` when the spec held 48 — the same mistake L4 records for the literal-site row, repeated in the row right beside it. The requirement is total coverage; the number is an accident of how many sub-requirements exist |
| **the defect's number** | folded `end_ms - start_ms == 1_461_000` ms **and** the rendered card value is exactly `24m21s` | `a_notification_ends_the_agent_it_names` + `a_background_agent_reports_its_real_duration_end_to_end` |
| **witness independence (R10.3)** | disabling either witness alone fails `>= 1` test, and the two failure sets are **disjoint** | temporarily neutralise one witness in `is_async_launch`, run `cargo test -p ai-monitor`, revert with `git checkout --`; repeat for the other |
| changed files | every path is in the **In** set — no bare count | `git diff --name-only \| grep -vE '^(crates/ai-monitor/(src/telemetry\.rs\|tests/telemetry\.rs)\|crates/app/src/dashboard\.rs\|specs/agent-elapsed-sidechain/(spec\|loop)\.md)$'` must print nothing. Measured `0` changed files at run-2 start, so the set below is exactly what this run added. A count was the run-1 form and is dropped for the same reason as the literal-site row (L4): the confinement is the requirement, the number is an accident of how many files a correct fix happens to need |
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

**Run-2 items (the four findings):**

- **Both fixes point the same way down D2** — "absent" and "unreadable" must resolve to the
  benign side, and only an *explicit* adverse signal to the failure side. Finding 4's fix has to
  read as the same idiom as `telemetry.rs:1166` (`is_error`: only an explicit `true` is a
  failure), not as a second, differently-shaped rule for the same question.
- **The spec amendment corrects the reasoning, not the score** — R7.1/R7.5/D13 must end up saying
  what is *true* of the fold, with the reason the original wording was wrong preserved. An
  amendment that merely deletes the sentence the code now violates is a gate failure: the next
  reader must be able to see that "written only by the notification branch" was a claim that got
  tested and lost, not a line that quietly vanished.
- **The two non-fixes are recorded with grounds, not with silence** — findings 2 and 3 were
  *decided*, not missed. Each needs its measured grounds and the D2 argument for keeping it, dated
  as an as-of reading (L3), so a later reader does not re-file them as undiscovered bugs. Finding
  2's record must state the widened window honestly rather than implying no cost.
- **Every new test names the mutation it catches (L2)** — for each assertion added, the mutation
  it claims to catch must actually fail it. In particular the finding-4 test must fail if
  `is_none_or` is reverted to `map_or(false, ..)`/`== Some("completed")`, and the finding-1 test
  must fail if `ended_ok = ok` is dropped from the non-launch path — verified by running the
  mutation, not by asserting it in a comment.
- **R7.5's "nothing else is ever Rose" survives narrowing** — the rule loses its sync-completion
  arm only for *failed* sync completions. A successful sync completion, a live agent and a D2
  fallback whose result carried no `is_error` must all still be non-Rose, and the existing test
  must still cover that.

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
  escalation with the evidence, not a quiet edit. Run 2 raised exactly that escalation and the
  human ruled on it, which is why R7.1/R7.5/D13 and the R7.3 GWT may be amended here — those four
  edits and no others. A fifth amendment is a fresh escalation.
- **Fixing findings 2 or 3.** Both were escalated and both were ruled *keep*. Preserving the
  `subagents` ring across `restarted()`, touching `lib.rs`, or narrowing the prose literal are all
  reversals of a human decision, not auto-fixes.
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

### L5 — a test that cannot fail is not a test; delete it and say so
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: When L2's mutation check finds that NO test fails, do not engineer a fixture until one does. First ask whether the mutation changes any observable output. If it does not, the guard is imaginary — delete the test rather than dress it up, and record the non-guarantee where the code is instead.
- note: run 2 added `a_launch_receipt_never_reports_on_the_run_it_started`, claiming to catch hoisting the `ended_ok` write above the `if !launched` gate. Running that mutation failed nothing, and the reason was not a weak fixture: a receipt carries no `is_error`, so the hoisted write stores the value already there, and an errored receipt leaves `completed: false`, which the accent renders Amber whatever `ended_ok` says. The mutation is unobservable in every reachable render, so no honest test could exist. The tempting repair — invent a receipt carrying `is_error: true`, a shape no log has shown — would have bought a green assertion by importing exactly the non-occurring-fixture problem L1 exists to stop. L2 asks why a test passes; this asks what to do when the answer is "nothing could make it fail".

### L6 — a rejection is about the proposal, not about the defect
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Never reuse an earlier round's "keep / won't fix" as grounds for rejecting a NEW proposal against the same defect. Re-measure against the proposal actually on the table, and say which alternatives the earlier ruling was made over.
- note: run 2 ruled the prose witness "keep" after measuring two proposals that both lost real detections — narrowing the literal, dropping the witness. Run 3 proposed a different shape (gate the prose on `toolUseResult` being absent) and the measurement separated cleanly: every prose-only detection lacks the object, every genuine sync answer carries one, so the gate lost nothing and made the false positive unreachable. Citing the run-2 ruling would have buried a strictly better option under a decision that had never seen it.

### L7 — establish that a reported defect is REACHABLE before fixing it
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: For each review finding, run the mutation that removes the proposed guard BEFORE writing its test. If nothing fails, determine whether the path is reachable at all. If it is not, keep the guard if it is cheap, record the unreachability, and do NOT manufacture a fixture to turn it green.
- note: the review reported an unconditional `agent_id` write as severing the notification join via a duplicate `tool_use`. Removing the guard failed no test, and the reason was structural, not thin coverage: a repeat `tool_result` never reaches the `Agent` arm because `pending.remove(tid)` already consumed the entry, and a duplicate `tool_use` pushes a fresh ring entry to the front that the lookup matches instead of the one holding the id. Writing a passing test would have meant inventing a shape the fold cannot produce — the L1/L5 failure mode arriving through a review finding rather than through a spec.

### L8 — a spec section can vanish inside an edit and no gate will notice
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `! comm -23 <(git show HEAD:specs/agent-elapsed-sidechain/spec.md | grep '^#\{2,3\} ' | sort -u) <(grep '^#\{2,3\} ' specs/agent-elapsed-sidechain/spec.md | sort -u) | grep -q .`
- note: inserting R3.3a meant matching the text that ended at `### R4:`, and the replacement did not put that heading back — so R4.1-R4.5 silently reparented under R3. Every gate stayed green: the compiler does not read markdown, the coverage audit walks `#### Rn.m` and never checks its parent, and the tests knew nothing. A separate Gate-3 checker found it, which is not a gate. The command above compares HEAD's `##`/`###` headings against the working tree and fails only on ones that DISAPPEARED, so legitimate new sections still pass — verified both ways at record time.

