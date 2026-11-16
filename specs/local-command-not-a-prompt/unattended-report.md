# Unattended run report — local-command-not-a-prompt

## Orchestration Complete

**Pattern**: Sequential Pipeline. The non-TT tasks form a sequential chain T1 → T2 → T3:
step_count 3, parallelizable 1, no inter-communication. TT1–TT4 were excluded from the
signals and ran first as one parallel batch.
**Tasks**: 7 completed (TT1–TT4, T1, T2, T3), 0 parked.
**Result**: Local commands are recognised in every `activity.rs` walk and in the digest, and
all 33 in-scope GWTs were judged met after 1 re-dispatch round. The run stopped at the
verification gate's Gate 3 because the one remaining fix would change an approved spec
decision. Nothing was committed.
**Verification**: escalated: Gate 3 safety bias — D1's text-prefix markers can skip a typed human prompt; the fix changes spec decision D1 (see `loop-escalation.md`)

- **Branch**: `fix/local-command-not-a-prompt` (HEAD `0686dac`, unchanged by the run)
- **Commit**: none. The run is not green, so the changes are left uncommitted for review.
- **Spec Review** (specify): PASS after 3 reviews, 0 open.
- **Parked tasks**: none.
- **`Status: assumed` decisions**: D2, D4, D5, D6, D7, D8, D10 (D1, D3, D9 and D11 are
  resolved from the request or research).

## Next step for the person

Read `specs/local-command-not-a-prompt/loop-escalation.md` and choose one of its two options:

1. **Amend D1 with a veto** (recommended). A record carrying `origin.kind: "human"` (or
   `turnOrigin`) is never a local-command caveat, output or wrapper. That is ~3 lines in
   `activity.rs` and one `tail_state` test. Measured: no record in the corpus changes reading.
2. **Accept the gap** and record it in the spec's Known Gaps.

Either way, the loop also lists doc-comment fixes it would have applied (Clarity 7/10).

**To run again:** edit the spec (for example amend D1), and a re-run starts over on its own.
A re-run on an unchanged spec stops at once, because the ledger records that this run ended.
To start over without editing the spec:

```
rm crates/ai-monitor/tests/tt1_local_command_tail.rs crates/ai-monitor/tests/tt2_local_command_walks.rs crates/ai-monitor/tests/tt3_local_command_wide_tail.rs crates/ai-monitor/tests/tt4_local_command_digest.rs specs/local-command-not-a-prompt/gwt-tests.md specs/local-command-not-a-prompt/requirement-check.md
```

Alternatively, keep the work: apply option 1 by hand on this branch, run `cargo test
--workspace`, and commit.

## Phase 3.4 — requirement check

Two checks ran (round 0 and round 1), each by a fresh-context checker. Both returns were
validated: one line per GWT, grounds matching the mapping, verdict/code pairs consistent.

| GWT | verdict | grounds | code | first met in round |
|-----|---------|---------|------|--------------------|
| R0.1 | met | read | ok | 0 |
| R0.2 | met | read | ok | 0 |
| R0.3 | met | read | ok | 1 |
| R1.1 – R1.14 | met | test | ok | 0 |
| R2.1 – R2.6 | met | test | ok | 0 |
| R3.1 | met | test | ok | 0 |
| R3.2 | met | read | ok | 0 |
| R4.1 – R4.6 | met | test | ok | 0 |

`grounds: test 29 / read 4`

- **Round 0:** 1 of 33 unmet. R0.3: the mutation survivor at `telemetry.rs:2026` sits inside
  the modified `digest_turn`, and the report neither killed it nor argued it equivalent.
- **Round 1:** dispatched T1, T2 and T3, each to a new worker. T1 and T3 changed nothing:
  the survivor is not in their files. T2 added `telemetry.rs::tests::the_background_cap_tracks_exactly_turn_bg_cap_ids`,
  which kills the survivor. The recheck found 0 of 33 unmet.
- **BLOCKED:** none. **FENCED:** none.
- **TT batch:** the snapshot check was clean (HEAD and index unchanged; the only new entries
  were the four listed `tt` files). Nothing was reverted or restored. Acceptance tests from an
  earlier spec: none.

## Phase 3.5 — verification gate (`harness-ops:loop`, unattended, contract `loop.md`)

- **Gate 1:** ✅ `cargo build`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check` and `cargo test --workspace` all exit 0; tests **2569 passed,
  0 failed**.
- **Gate 2:** ✅ all 4 TT file hashes match `gwt-tests.md`; mutation survivors in the changed
  functions: 0 (both runs postdate the files' last edits); `ai-monitor` test failures: 0.
- **Gate 3** (scored by a separate checker subagent):
  - Spec alignment 9/10 — meets the bar.
  - **Safety bias 7/10** — below the bar of 8. The text-prefix recognisers would skip a typed
    human prompt that begins `<local-command-stdout>` / `-stderr` / `-caveat` and read the
    record before it, a possible false `Waiting`. Measured 0 such prompts. Fixing it changes
    D1, which is a 🛑 spec-conflict boundary, so the loop escalated.
  - **Clarity 7/10** — below the bar. The `state_from_reads` doc comment is outdated, and a
    few recognisers lack decision citations. This is Auto-fixable but was not applied,
    because the run stopped.
- **Lesson recorded:** `loop.md` L1 (`auto-unattended`, gate3-item).
- **No recheck after the gate:** the gate did not pass.

## Mutation coverage (R0.3)

Run 2026-11-15 with `cargo mutants -j 4 --test-package ai-monitor` (narrowed to the crate's
own tests; narrowing can only add false survivors, never false kills).

| file | scope (`--re`) | mutants | caught | unviable | missed |
|------|----------------|---------|--------|----------|--------|
| `crates/ai-monitor/src/activity.rs` | `last_meaningful`, `lines_newest_first`, `local_command_flags`, `without_local_commands`, `is_turn_record`, `is_local_caveat`, `is_local_output`, `is_user_wrapper`, `state_from_reads`, `newest_is_notification`, `skip_handed_back`, `background_pending`, `is_human_prompt` | 83 | 79 | 4 | **0** |
| `crates/ai-monitor/src/telemetry.rs` | `digest_turn` (the regex also matches `ag_digest_turn`) | 24 | 22 | 0 | **2** |

Re-run after round 1 (`telemetry.rs`, same scope, output in the session scratchpad
`mut-t2r1`): **24 mutants, 23 caught, 1 missed**.

Survivors in the changed functions (`digest_turn` and every `activity.rs` function above):
**0**.
- `telemetry.rs:2026` `replace < with <= in SessionTelemetry::digest_turn` (the
  `turn_bg.len() < TURN_BG_CAP` bound) survived the first run. It was a real gap, not an
  equivalent mutant: under `<=` the digest tracks 65 background ids where it means 64. It is
  **killed** by the round-1 unit test `telemetry.rs::tests::the_background_cap_tracks_exactly_turn_bg_cap_ids`.
  That test replays `TURN_BG_CAP + 1` background starts and pins the cap from both sides:
  with 63 stopped the turn stays open, with 64 stopped it ends. All three mutants on that
  line (`==`, `>`, `<=`) are caught.
- `telemetry.rs:2104` `replace < with <= in SessionTelemetry::ag_digest_turn` still survives.
  It is the same cap in the Antigravity fold, a function this change did not modify; the
  `--re digest_turn` pattern matches it only by substring. Outside R0.3's "changed
  functions", and left as found.

## Resolution after the escalation (2026-11-16)

The person chose option 1 (the D1 veto) and asked for a commit. The commit was made by hand,
at the person's instruction, not by the unattended pipeline. Changes since the escalation:
- `activity.rs`: `started_by_a_person` (`origin.kind: "human"` or any `turnOrigin`), refused
  by `is_local_caveat`, `is_local_output` and `is_user_wrapper`. Three unit tests pin it,
  one per case: the counter-example reads `UserPrompt`, a stray caveat before a skill
  wrapper, and `turnOrigin` alone. Also the Clarity fixes: `state_from_reads`' doc lists
  all three wider-read triggers, `is_turn_record` is documented, decision citations, and
  `gate` is renamed to `classified_from`.
- `docs/architecture.md` G8: the veto paragraph, and "leaves only the caveat".
- Spec: D1's veto paragraph, R1.15, a Known Gap for pre-2.1.300 logs, and `Amended` in Meta.

Verification after the change:
- `cargo build`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --all -- --check` exit 0; `cargo test --workspace` passes **2572, failed 0**.
- Mutation run on `started_by_a_person`, `is_local_caveat`, `is_local_output`,
  `is_user_wrapper`, `is_turn_record` and `state_from_reads`: 33 mutants, 32 caught,
  1 unviable, **0 missed**.
- All 4 acceptance-test files still match their recorded hashes.
- Gate 3 was not re-scored by a fresh checker after the veto. The two items below the bar
  were addressed by the person's choice and by the listed doc fixes.
