# loop.md — Loop Contract

> Verification contract for `specs/ai-monitor-absorbed-notifications/spec.md`, run as
> agent-orchestrate's Phase 3.5 gate after tasks T1-T6. Approved by the user 2026-09-28.

## Goal
The uncommitted implementation on `fix/ai-monitor-absorbed-notifications` satisfies every
sub-requirement R0.1-R9.1 of `specs/ai-monitor-absorbed-notifications/spec.md` without
violating its Decisions D1-D10 or its Constraints.

## Scope & non-goals
- In: `crates/ai-monitor/src/telemetry.rs`, `crates/ai-monitor/tests/telemetry.rs`,
  `crates/ai-monitor/tests/fixtures/absorbed_notifications.jsonl`, spec prose
  (`specs/ai-monitor-absorbed-notifications/spec.md`, `specs/agent-elapsed-sidechain/spec.md`).
- Out: `crates/app`, the three pre-existing fixtures, `TelemetrySnapshot`'s field list, any other
  crate, and git history (no commits).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| typecheck | N/A — Rust type-checks as part of the build |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |
| contract (R5.4, R8.1) | `git diff --quiet main -- crates/app crates/ai-monitor/tests/fixtures/session.jsonl crates/ai-monitor/tests/fixtures/context_command.jsonl crates/ai-monitor/tests/fixtures/antigravity_transcript.jsonl` |
| prose (R7.1, joined text) | `python3 -c "import re,sys; j=re.sub(r'\s*\n\s*(//[/!]?)?\s*',' ',open('crates/ai-monitor/src/telemetry.rs').read()); sys.exit(int('the ONLY signal a background agent' in j or 'the other is the \`queued_command\` attachment' not in j))"` |
| fixture guard (R5.3) | `! grep -qE '"isAsync" ?: ?false' crates/ai-monitor/tests/fixtures/absorbed_notifications.jsonl` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| in-diff mutants missed (R6.1) | `= 0` | `git diff main -- crates/ai-monitor/src/telemetry.rs > <scratch>/change.diff && cargo mutants -f crates/ai-monitor/src/telemetry.rs --in-diff <scratch>/change.diff -j 4 --cargo-test-arg -p --cargo-test-arg ai-monitor` |
| whole-file mutants missed (R6.2) | `≤ 43` | T6's run (`cargo mutants -f crates/ai-monitor/src/telemetry.rs -j 4 --cargo-test-arg -p --cargo-test-arg ai-monitor`), reused only if `telemetry.rs`'s mtime precedes that run's start; otherwise re-run |
| sub-requirements with evidence | `39 / 39` | each R*n.m* mapped to a named passing test or a Gate-1 check |

## Gate 3 — Qualitative  (score + grounds + action; checker-scored)
- **Spec alignment:** D1 (`commandMode` unread; `rendered*` unread), D2 (no outer starts-with in
  `on_attachment`), D3/D6 (handlers get the record's `ts`, never the clock), D5 (`.max()`,
  `first_event_ms` first-wins, both dialects), D10 (no contract change).
- **Test honesty:** assertions match their GWT with exact values; fixture tests cannot pass
  without the new code.
- **Comment accuracy:** new comments describe the code as it is, at the file's density.
- **Fixture fidelity:** verbatim structure, trimmed prose, no private free text.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests, docs/comments,
local renames, behaviour-preserving refactors.

**🛑 STOP — call a human (escalate):**
- Anything conflicting with the spec's Decisions or this contract's Goal
- Any change to `crates/app` or to `TelemetrySnapshot`'s fields
- DB schema / data-loss migrations / auth / payment / security (not expected here)

## Exit criteria
The loop exits only when **every** gate above passes and the Evidence Report is emitted.
Hitting a 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Append entries below. A fresh contract has none. -->

### L1 — Fixture validity is a Gate-1 fact, not a Gate-3 judgement
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `jq -c . crates/ai-monitor/tests/fixtures/absorbed_notifications.jsonl >/dev/null`
- note: run 1's checker failed Gate 3 (4/10) on "raw newlines" that zsh `echo` had injected by expanding `\n`; the file was valid. A byte-faithful parse settles it mechanically.

### L2 — Restore a falsified file without preserving its mtime
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: restoring with `cp -p` kept the pre-sabotage mtime, so cargo skipped the rebuild and tests ran the sabotaged binary (6 false failures); restore with plain `cp`, or `touch` afterwards.
