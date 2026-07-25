# loop.md — Loop Contract

> Supervisor document for **tab-identity** (verification of the already-complete
> T1→T2→T3 implementation). Defines "done" so the loop verifies itself against
> `specs/tab-identity/spec.md` ## Requirements. Implementation is complete — the
> loop starts by verifying, not by re-doing the work.

## Goal
Verify the tab-identity feature (per-tab live "current task" label from the session's
latest substantive Claude user prompt) matches `specs/tab-identity/spec.md`
## Requirements (R0–R5) and ## Constraints, without regressing the build/test/lint gates.

## Scope & non-goals
- In: app (`crates/app/src/tabmeta/*`, `sidebar_model.rs`), renderer
  (`crates/renderer/src/sidebar_row.rs`), ai-monitor (`crates/ai-monitor/src/*`).
- Out: `term-core` (must stay untouched — dependency-direction), waiting-area logic,
  card height/layout geometry, any GUI-crate dependency.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — folded into `cargo build` (rustc typechecks) |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| term-core untouched | `git diff --name-only | grep -q '^crates/term-core/' && exit 1 || exit 0` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| new tab-identity unit tests passing | `= 8` (all) | `cargo test -p ai-monitor -p app` count of `prompts_*` + `label_*`/`pick_label_*` tests |
| idle warm-log re-reads | `= 0` | `activity::tests::unchanged_log_is_not_reread` (asserts `LOG_READ == 0`, `LOG_STAT == 1`) passes |

## Gate 3 — Qualitative  (score + grounds + action; checker subagent)
Score each /10. A score alone is invalid — grounds + action mandatory.

- **Spec alignment (R1–R5, D1–D10)** — label = latest *substantive* user prompt; tool-result
  turns excluded; short/continuation fallback (D10); extraction-failure chain (D8); idle
  keeps last label (D3/D6); non-Claude/pure-shell → cwd (D7); same-cwd shared-log limitation
  (D4/R5).
- **Architecture fit** — term-core untouched; reuses the tabmeta worker tick + generation
  snapshot + MetaSource pull (no new thread/runtime); no GUI crate added.
- **Naming / readability** — `TabMeta.task`, `pick_label`, `is_substantive`, `label_of_cwd`,
  the RowMeta line-1/line-2 draw split are clear at their call sites.
- **Failure-mode robustness** — carry-forward caching and the fallback chain cannot panic or
  blank a card; the D10 policy lives app-side (not in ai-monitor).

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests,
docs/comments, local renames, behaviour-preserving refactors that keep Gate 1 green.

**🛑 STOP — call a human (escalate):**
- DB schema changes · data-loss migrations · auth/permission changes ·
  payment/security-sensitive changes
- Anything conflicting with this contract's Goal or `spec.md` (e.g. a change to
  `term-core`, card geometry, or waiting-area behaviour)

## Exit criteria
The loop exits only when **every** gate passes and the Evidence Report is emitted.
Hitting a 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Append entries below. A fresh contract has none. -->

### L1 — Claude Code command-wrapper turns are not human prompts
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `cargo test -p ai-monitor prompts_skip_claude_code_command_wrappers`
- note: real logs carry synthetic role=user turns (`<command-name>…`, `<local-command-stdout>…`) that clean test fixtures lacked; they were leaking into tab labels until `is_command_wrapper` filtered them. Real-data (/run-term) eyeballing caught what unit fixtures missed.

### L2 — label needs a wider read than the 8 KiB activity tail
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `cargo test -p ai-monitor recent_prompts_finds_a_human_prompt_far_beyond_the_activity_tail`
- note: the label reused the 8 KiB `tail_state` window, but a live session's last human prompt sits far behind the newest activity (one prompt + >8 KiB of assistant/tool output) → label was always None in the running app. Split into a wider `recent_prompts` (up to 1 MiB) scan. Clean fixtures (all <8 KiB) never exposed it.

### L3 — 1 MiB label re-read amplification on a hot, large log
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: on an active tab whose log exceeds 1 MiB and appends every tick, each change re-reads up to 1 MiB (carry-forward fixes staleness, not this I/O). Follow-up: scan only the newly-appended region or throttle the wide re-scan. Non-blocking for v1.
