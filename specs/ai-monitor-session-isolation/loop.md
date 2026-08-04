# loop.md — Loop Contract: ai-monitor-session-isolation

## Goal
**Round 2 (2026-08-01, from a live defect report).** Tier 2 must not match a log whose
first event predates the tab's `claude` process. Observed: app pid 4996 (fixed binary),
tab 1's claude pid 5550 started 11:38:25 and wrote `c2abd08b` at 11:38:26; tab 2's claude
pid 5771 started 11:38:41 and wrote nothing. `|11:38:26 − 11:38:41| = 15s ≤ 120s`, so the
symmetric window confirmed tab 1's log as tab 2's own, `owned: true`, non-empty — D4 then
attached permanently. Both tabs rendered one dashboard.

The missing invariant: **a session cannot write an event before the process that creates
it exists.** The window becomes tight below (clock skew only) and stays 120s above
(human-approved 2026-08-01: wrong-but-honest beats wrong-and-confident).

Round 1 goal (verifying the original implementation) passed all gates; see git history.

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
| the reported timeline reproduced | 1 named test, passing | proc A at `T` with a log whose first event is `T+1s`; proc B at `T+16s` with no log of its own → `resolve_owned_session` for B returns `None`, NOT A's log |
| protected fallback-test bodies edited | `== 0` of 5 | extract each named fn body from `git show HEAD:crates/ai-monitor/src/lib.rs` and from the working tree, byte-diff |
| `.snap` files changed | `== 0` of 9 | `git status --porcelain crates/app/src/snapshots/` prints nothing |
| clippy warnings | `== 0` | the Gate-1 lint command |
| existing tier-2 tests | all pass, bodies unedited | `two_tabs_on_one_project_get_their_own_sessions`, `a_session_started_by_this_tab_beats_a_more_active_foreign_one`, ~~`the_upper_bound_is_the_same_120s_as_the_lower`~~, ~~`a_session_that_began_just_before_the_process_start_still_qualifies`~~ — **STALE, see note** |

> **Note (2026-08-03, from `specs/ai-monitor-rival-assignment`).** Two of the four tier-2
> tests named above **no longer exist**: `the_upper_bound_is_the_same_120s_as_the_lower`, and
> `a_session_that_began_just_before_the_process_start_still_qualifies`, which was replaced by
> `a_log_that_predates_the_process_is_not_that_process_s_session` when the window became
> asymmetric. A gate that greps for a test that is not there passes vacuously, so this row
> was silently checking two of four. Corrected here rather than in place so the original
> contract stays readable; the current protected set is enumerated by name in
> `specs/ai-monitor-rival-assignment/spec.md` § Constraints, against the tree as it is.

## Gate 3 — Qualitative

Scored by a **separate checker subagent** (maker ≠ checker); `score/10 + grounds + action`.

1. **Invariant, not a constant.** Does the new bound encode "a session cannot predate its
   process" as a stated rule, rather than a tuned number that happens to fix one report?
2. **Idle-at-splash is handled or recorded.** A tab where `claude` runs but the user does
   not type for >120s: the first event lands outside the upper bound. Handled, or written
   into Known Gaps with its recovery path (the watcher re-resolves on the first write)?
3. **Known Gap #4 is corrected.** It currently reads "Pre-existing, unchanged by this
   work" — the defect report disproves that. Does it now state what actually happens and
   that D4 made it permanent?
4. **L1 (lesson) — mutation-tested.** Is the new lower bound pinned by a test that FAILS
   when the bound is reverted to symmetric? Verified by mutation, not by reading.
5. **L2 (lesson) — premise measured.** Is the new bound's justification measured against
   real data, with the measurement recorded next to the constant? The previous round's
   symmetric window was approved on an *asserted* premise and this is the result.
6. **L3 (lesson) — fallback classes enumerated.** Does the change re-check what input
   classes the tightened rule now rejects that it previously accepted, and cover or
   record each?

---

## Autonomy Boundary

**✅ Auto-fix:** the tier-2 bound in `crates/ai-monitor/src/lib.rs`, its tests and docs,
and the Known Gaps / Decisions text in `specs/ai-monitor-session-isolation/spec.md`.

**🛑 STOP — escalate:**
- Changing `Wanted`'s shape to carry new process data (a design change, not a bound fix).
- Any change to `crates/renderer/**` or `crates/common/**`; any new crate dependency.
- Widening the upper bound beyond 120s (the human chose this value on 2026-08-01).
- Weakening or deleting an assertion to make a gate pass; editing a protected test.
- Accepting a changed `.snap`.

## Exit criteria
Every gate passes and the Evidence Report is emitted.

## Lessons

### L1 — a test passing for two independent reasons pins neither
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: For every coordination primitive this feature relies on (the shared `attached` flag above all), is there a test that FAILS when the primitive is removed? Verify by mutation, not by reading — disable the primitive, run the suite, and require exactly the intended test to fail.
- note: After resolution went strict, all three watcher freeze-tests began passing for the wrong reason (the rival was refused by `resolve_owned_session`, not by the flag). Disabling the guard entirely left the suite green — the flag could have been deleted with no CI signal. Fixed by `a_frozen_watcher_refuses_a_log_its_hint_would_otherwise_confirm`, whose hint NAMES the rival so only the flag can stop the switch.

### L2 — a premise cited to justify a decision must be measured before approval, not after
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: For each decision in the spec whose rationale rests on a factual claim about runtime behaviour, is that claim measured and the measurement recorded next to the constant/code it justifies? A claim stated as fact in a rationale but never run is a finding.
- note: D3's ±120s window was approved on the asserted premise that a session writes its first event "seconds after the process". True — 1s/2s/1s/2s, n=4 — but only measured at verification, one layer after the human approved it. **Round 2 shows the cost was real, not hypothetical**: the premise was true and still insufficient, because it was never checked against TWO tabs opened seconds apart, which is the ordinary workflow. Measuring the premise is not enough — it must be measured against the scenario the decision will actually meet.

### L3 — when strict logic replaces a fallback, enumerate what the fallback was getting right by accident
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: List every input class the removed fallback handled correctly — including the ones it handled correctly *by coincidence* rather than by design — and confirm the strict replacement either covers each or records it as a Known Gap.
- note: Deleting the tier-3 ranked fallback from the production path silently broke `claude --continue`: the fallback ranked the most recently active log, which is by definition the session `--continue` resumed. Accuracy by luck, but accuracy — and its loss makes the panel state something false. Found only because a checker re-derived the resolver's input space; no gate would have caught it.

### L4 — a "pre-existing, unchanged by this work" gap must be re-derived, not asserted
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: For every Known Gap dismissed as pre-existing or unchanged, re-derive it against the NEW code: does the change alter its likelihood, its permanence, or its visibility? A gap that was survivable under a fallback may be terminal under a strict rule.
- note: Known Gap #4 ("two sessions started within 120s cannot be separated") was written off as pre-existing. It was not unchanged: D4 made it attach permanently and immediately, and D3's symmetric window made a *physically impossible* match (log first event 15s BEFORE the process started) admissible. It then fired on the first real two-tab test — 16 seconds apart, the ordinary workflow.

### L5 — an environment variable that a process inherits cannot identify that process
- cause: escalation
- source: human-approved
- form: context
- status: active
- check: —
- note: `CLAUDE_CODE_SESSION_ID` was proposed as a clean per-process session identifier and offered to the human as a follow-up fix. It is purely INHERITED: a user-launched `claude` never sets it for itself, so it names whichever ancestor session spawned the process — the exact bug class this feature exists to remove. Proven by launching the app with the var stripped: both tab `claude` processes then had no such var at all. Do not re-propose it.
