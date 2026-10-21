# Unattended run report: agy-background-working

> **Superseded 2026-10-20:** the user approved the spec and it was implemented interactively.
> See `spec.md` › `## Verification`. This report records the unattended stop only.

- **Branch**: `fix/agy-background-working` (worktree `../terminal-fix-agy-background-working`), nothing committed, nothing pushed
- **Stopped at**: the L4 Spec Review gate, before execution. No code was written.
- **Why**: the 3rd and final fresh-context Spec Review returned NEEDS_FIX. Its one issue was a
  labelling inconsistency: the Tasks preamble listed R2.1/R2.2 as "green before T1", but their
  positive-control cases can only pass after T1. That is now fixed in the spec, but the fix has
  not been re-reviewed, because the review budget (3) was spent. Unattended Mode stops here
  by rule.

## What the spec decides
An agy card reads **Working** after a tool-less reply when a background task or subagent
started in that turn has not yet reported back. Pairing works from transcript strings: a
`RUNNING` start `…task id: <uuid>/task-<n>` or a `Created the following subagents:` header,
matched to a `SYSTEM_MESSAGE` from `sender=<id>` or carrying `Task id "<id>" …`. When nothing is
pending, behaviour is exactly today's. This amends agent-card-status D13.

Measured on 120 transcripts (2026-10-19 07:10 UTC): 48/48 mid-work replies are pending, and 2 of
52 turn ends after background work would read Working (one was an agent-launched `cargo run`
still running).

## Assumed (no person decided these)
- D4: the walk boundaries (last `USER_INPUT`; an unreadable line keeps a pending start found newer than it).
- D5: reuse `AssistantBusy`; no new variant.
- D6: no time cap. A hand-off after launching a long-lived process reads Working until it exits or the user types.
- D3: any message from a task or subagent id counts as its end (57/57 task messages measured terminal).

## Would be parked
Nothing. No task is fenced.

## Next step for you
Read `specs/agy-background-working/spec.md` (Decisions D1–D8, Requirements R0–R4). If it reads
right, approve it, and execution runs TT1–TT4 → T1 ∥ T2 → T3 (mutation gate + corpus re-measure).
