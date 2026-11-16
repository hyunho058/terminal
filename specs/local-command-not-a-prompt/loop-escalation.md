# Loop escalation — local-command-not-a-prompt (2026-11-15)

**Gate:** 3 (qualitative), iteration 1. Gates 1 and 2 passed.

**Boundary:** a fix that changes an approved spec decision (D1) — the loop's 🛑 "conflicts
with the approved spec" fence. Not auto-fixed.

**What the checker found (Safety bias 7/10, bar 8):** `is_local_output` / `is_local_caveat`
match on the text prefix alone, so a typed human prompt that *begins* with
`<local-command-stdout>` (or `-stderr`, `-caveat`) is skipped and the record before it decides:

```
{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}}
{"type":"user","origin":{"kind":"human"},"message":{"role":"user","content":"<local-command-stdout>Set model to opus</local-command-stdout> why did it pick this?"}}
```
New code → `AssistantEndTurn` (Waiting); HEAD → `UserPrompt` (Working). Claude answers it. That
is a false Waiting, which the spec's Constraint 1 forbids, but D1 defines the markers by text
only. The spec contradicts itself here; resolving it is a spec decision.

**Evidence (2026-11-15T01:05Z, `~/.claude/projects`):** 0 user records with
`origin.kind: "human"` begin with any `<local-command-…>` tag. All 27 `<local-command-stdout>`
and 38 `<local-command-caveat>` user records carry `origin: null`, no `turnOrigin`. So the path is
unobserved, and a veto would change no measured record.

**Options for the person:**
1. **Amend D1 with a veto** (recommended): a record carrying `origin.kind: "human"` (or
   `turnOrigin`) is never a local-command caveat, output or wrapper. ~3 lines in
   `activity.rs` (`is_local_caveat`, `is_local_output`, `is_user_wrapper`), one `tail_state`
   test for the log above expecting `UserPrompt`, and a Known Gap for pre-2.1.300 logs (no
   `origin` field) where the path stays open. It also closes the checker's second, unobserved
   case (an orphan caveat before a skill wrapper; 39 of 42 skill wrappers carry
   `origin.kind: "human"`).
2. **Accept the gap**: record it in the spec's Known Gaps (0 measured) and keep D1 as is.

**Also found, Auto-fixable (Clarity 7/10), not applied because the run stopped here:**
`state_from_reads`'s doc comment (`activity.rs` ~99–101) still says only an `end_turn` reads past
the tail — false after D9 (and already incomplete for D27). Rewrite it to list the three
wider-read triggers sharing one read; add a doc line to `is_turn_record` (D7); cite D1/D8 on
`is_local_output` and D1/D3 on `is_user_wrapper`; in `docs/architecture.md` change "leaves the
caveat alone" to "leaves only the caveat".

## Resolution (2026-11-16)

The person chose **option 1** and asked for a commit. D1 was amended with the veto (spec
`## Meta` › Amended), and R1.15 added. `activity.rs` gained `started_by_a_person`, which
`is_local_caveat`, `is_local_output` and `is_user_wrapper` now refuse. Three unit tests pin it:
the counter-example above now reads `UserPrompt`, a stray caveat before a skill wrapper keeps
it a prompt, and `turnOrigin` alone vetoes. The Clarity fixes listed above were applied in the
same change (`state_from_reads`' doc comment, `is_turn_record`'s doc, decision citations, the
G8 wording, and `gate` renamed to `classified_from`).
