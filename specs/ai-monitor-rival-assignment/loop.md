# loop.md — Loop Contract: ai-monitor-rival-assignment

## Goal
Verify the **already-complete** implementation of `specs/ai-monitor-rival-assignment/spec.md`
(Status: approved) against its `## Requirements` — R0–R8, 34 GWT sub-requirements.

The change: `resolve_owned_session`'s tier-2 rival test could hand a tab's own log to a
different tab (`owned: true`, latched permanently by `Attachment::admit`) while leaving the
tab that actually wrote it empty. Rivals now carry `(pid, started_ms)`, are disqualified by
their own registry record, and the uniqueness rescue fires only on an **effectively- and
known-empty** rival set.

Implementation is finished (T1–T6). This contract verifies; it does not rebuild.

---

## Gate 1 — Pass/Fail (binary, 100%)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| snapshots untouched (R0.3) | `test -z "$(git status --porcelain crates/app/src/snapshots/)"` |
| protected bodies (R8.1) | `./specs/ai-monitor-rival-assignment/check-protected-bodies.sh` |

**On the protected-bodies check.** It diffs the five `resolve_session` fallback tests against
`HEAD` while normalising away the `&[]` → `&[], true` arity retype that Done-when #4 permits.
This distinction is the whole point: the sibling contract
(`specs/ai-monitor-session-isolation/loop.md:34`) byte-diffs whole function bodies, which
cannot tell a permitted retype from a changed assertion and would fail on both.

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| tests passing | `>= 1023` | sum of `test result: ok. N passed` (baseline before this work: 1014) |
| `.snap` files changed | `== 0` of 9 | `git status --porcelain crates/app/src/snapshots/` prints nothing |
| protected bodies with a MEANING change | `== 0` of 5 | the Gate-1 script above |
| mutation kills on the four new rules | `== 4` of 4 | invert each rule in turn; each must be killed by ≥1 test |
| additional mutations attempted | recorded, with survivors named | extra probes beyond the four; a survivor is a coverage limit and must be stated, not hidden |
| sub-requirements with NO automated coverage | `<= 3`, **each named** | enumerate against R0–R8; silence is not coverage |
| sub-requirements only PARTLY covered | **each named** (no threshold) | a test that exercises one branch does not cover the other |

**Uncovered, as of the iteration-3 re-audit** (the first enumeration was wrong in both
directions — it listed R5.4, which *is* covered by
`each_rival_registry_is_read_once_per_resolution_not_once_per_candidate`, and omitted R6.1
and R6.2):
> **By-construction claims below are INSPECTION-ONLY.** M8 showed a by-construction
> argument about `open()` was wrong for a full iteration; the same class of claim is
> asserted, unverified, for R6.1, R6.4 and R1.2's "safe by construction".

- **R6.1** records re-read while unattached — *inspection only*: true by construction (`rival_claims` runs on
  every `resolve_owned_session` call) but no assertion pins it.
- **R6.3** refresh stops once attached — the guard exists (`refresh_rivals` returns early on
  `attached_flag`), but nothing drives it.
- **R6.4** refresh never on the event-loop thread — *inspection only*; structural.

**Partly covered:**
- **R6.2** membership republished, both directions. The mechanism is real and tested at two
  levels — `publish_rivals_into` (M7 kills the orphaned handle) and `watch_with_rivals`
  (the `Watch` carries the stored allocation). But **`open()`'s use of those seams is not
  under test**: `open()` needs an `EventLoopProxy`, so it cannot be driven from a unit test,
  and **M8 — call the publisher, discard its return, hand `Watch` a fresh `Arc` — survives
  both clippy and all 472 app tests**. That is the exact iteration-2 defect, still
  representable. Measured, not assumed: moving the seam one level up was tried and did *not*
  close it. Closing it for real needs `open()` itself to be testable, which is a larger
  refactor than this change should carry.
- **R1.2** only the "the other session's log does not exist" branch is tested; the "does
  exist" branch is *inspection only*: safe by construction (no `is_file()` check exists),
  but unasserted.
- **R2.4** `>1` surviving candidate never reaches the rescue — unreachable by construction,
  and the existing two-candidate test has both inside the slack, so the branch that matters
  is never exercised.

**The mutations** (each must turn exactly one test red):
1. gate the rescue on the raw rival list instead of the effective set
2. drop the `rivals_known` gate
3. compare on the filename stem only (drop D13's content session id)
4. trust a stale rival record (drop the drift guard)

**Additional probes** (beyond the contract's four):
- M5' remove `candidate_identity`'s stem fallback → killed by
  `an_unreadable_content_id_falls_back_to_the_stem_and_keeps_the_veto`
- M6 read rival registries per-candidate → killed by
  `each_rival_registry_is_read_once_per_resolution_not_once_per_candidate`
- M7 orphan the published handle → killed by
  `publish_rivals_stores_the_same_handle_it_hands_to_the_watcher`
- **M8 discard the publisher's return in `open()` and substitute a fresh `Arc` → SURVIVES.**
  Recorded as the R6.2 coverage limit above rather than presented as a kill.

Score: **7 killed of 8 attempted**; the four contract mutations are 4 of 4.

## Gate 3 — Qualitative

Scored by a **separate checker subagent** (maker ≠ checker) given only the diff, this
contract, the spec, and the changed file paths — never the maker's reasoning.
Each item: `score/10 + grounds + action`. A bare score is invalid and fails the gate.

1. **The regression test is real.** Does
   `the_writer_keeps_its_log_when_an_idle_registered_sibling_cannot_have_written_it`
   actually encode the reported timeline (A@T, idle registered B@T+28s, sole log at T+900s),
   and does the effective-set gate genuinely fix it — or does it pass for an incidental
   reason?
2. **`refresh_rivals` is bounded.** It spawns a thread per metadata sync
   (`crates/app/src/overlay.rs`). Leak, storm, or acceptable? This is the loosest part of the
   change and the maker said so up front.
3. **Uncovered requirements are named, not hidden.** R5.4, R6.3 and R6.4 have no direct
   automated coverage. Are they honestly recorded, or does the suite imply coverage it
   doesn't have?
4. **The R5.3 amendment is sound.** R5.3 originally demanded byte-unchanged call sites and was
   amended mid-implementation when the known/unknown flag had to reach the resolver. Sound
   reasoning honestly recorded, or a bar quietly lowered to fit the code?
5. **Spec alignment.** Is any of D1–D13 silently unimplemented or contradicted by the code?

---

## Autonomy Boundary

**Auto-fix (stay in the loop):** lint/format errors, missing tests for code already in scope,
doc and comment corrections, local renames, behaviour-preserving refactors that keep Gate 1
green.

**STOP and ask:** anything that conflicts with the approved spec. This is a live risk rather
than boilerplate here — R5.3 was already amended once during T1, and the spec's own Review
Record notes that D12 and D13 were added on the L2 reviewer's final allowed pass and were
never themselves reviewed.

## Lessons

### L1 — the rival handle the watcher reads must be the one the overlay stores
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `./specs/ai-monitor-rival-assignment/check-rivals-wiring.sh`
- note: `open()` built its own handle and never assigned `self.rivals`, so `refresh_rivals`
  wrote where nothing read and D11 was inert in production — past 1025 tests, clippy and fmt.
  Verified load-bearing: exits 1 on the M8 mutation the whole suite misses. Root cause of the
  original miss was a scripted `str.replace` that silently no-matched after `cargo fmt`
  reformatted its target between write and patch.

### L2 — a comment may only claim the verification that exists
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Does every comment asserting that a test or mutation catches something **name the
  mutation it actually catches**, and is that claim true? Three consecutive iterations of
  this loop shipped a comment claiming more verification than existed — "unrepresentable"
  (false), then crediting `watch_with_rivals` with catching M8 (it catches M9).
- note: an overclaiming comment is worse than none; it tells the next reader a hole is closed.

### L3 — apply the `<7` rule to the checker's own scores
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: the Gate-3 checker twice wrote a verdict line that contradicted its own scores
  (declared PASS while scoring an item 6/10). The contract's threshold governs, not the
  checker's summary. Read the scores, not the verdict.
