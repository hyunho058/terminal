# loop.md — Loop Contract

> The supervisor document for `agent-card-status` (post-execution verification gate).
> Defines what "done" means for the T0–T5 implementation so the loop verifies it instead
> of trusting it.

## Goal
The uncommitted implementation on `feat/improve-card-ui` satisfies every sub-requirement
(R0.1–R9.3, 57 in all) of `specs/agent-card-status/spec.md`, and violates none of its
Decisions, Constraints or Non-goals; where the code departs from a decision's wording, the
departure is recorded in the spec's `## Implementation Notes` and does not change what the
decision decided.

## Scope & non-goals
- In: the files T0–T5 touched —
  `crates/ai-monitor/src/{antigravity.rs,antigravity/tail.rs,telemetry.rs}`,
  `crates/ai-monitor/tests/antigravity.rs`;
  `crates/renderer/src/{sidebar_card,sidebar_row}.rs`, `crates/renderer/tests/golden_frame.rs`;
  `crates/app/src/{prompt,session,sidebar_model,tabs,terminal_app}.rs`,
  `crates/app/src/tabmeta/{antigravity,worker,activity,mod}.rs`;
  `docs/architecture.md` (G8); the spec and its `captures/` — and tests for code already in them.
- Out: the card's look, the ai-monitor panel, agent detection, Codex/Gemini/Aider real state
  (the spec's Non-goals); `Cargo.lock` and every `Cargo.toml`; commits; `~/.gemini` (read-only);
  the `/run-term` eyeball check (a human Post-work step); the workspace-trust marker (gated on a
  capture that has not been taken — D12).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust: `cargo build` is the type check |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |
| GPU tests really ran (R5.5) | `test -z "$SKIP_GPU_TESTS"` — with it unset, `testing::headless` fails rather than skips, so a green golden frame means it rendered |
| term-core stays GPU/window-free (C4) | `! cargo tree -p term-core -e normal --prefix none \| grep -qE '^(wgpu\|winit) '` |
| ai-monitor gains no FD code (AG11, R9.3) | `! grep -rnE 'libc\|proc_pid' crates/ai-monitor/src \| grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' \| grep -q .` and `! cargo tree -p ai-monitor -e normal --prefix none --depth 1 \| grep -q '^libc '` |
| no dependency added, lockfile untouched (R9.3) | `git diff --quiet HEAD -- Cargo.lock Cargo.toml 'crates/*/Cargo.toml'` |
| the fixture is the capture (D12) | the `REAL_ANTIGRAVITY_DIALOG` literal in `crates/app/src/prompt.rs` is byte-identical to `specs/agent-card-status/captures/antigravity_permission_dialog.txt` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| unargued mutation survivors, `ai-monitor/src/antigravity/tail.rs` | `= 0` | `cargo mutants -f crates/ai-monitor/src/antigravity/tail.rs --cargo-test-arg -p --cargo-test-arg ai-monitor` |
| unargued mutation survivors, `app/src/tabmeta/antigravity.rs` | `= 0` | `cargo mutants -f crates/app/src/tabmeta/antigravity.rs --cargo-test-arg -p --cargo-test-arg app --cargo-test-arg --lib` |
| unargued mutation survivors, app diff | `= 0` | `cargo mutants --in-diff <git diff HEAD of the app files in scope>` with `-p app --lib` |
| unargued mutation survivors, renderer card | `= 0` | `cargo mutants -f crates/renderer/src/sidebar_card.rs` with `-p renderer --lib --test golden_frame` |
| sub-requirements with a named, passing verifier | `57 / 57` | traceability matrix: each R-id → the test (or Gate-1 command) that checks it, each test confirmed to exist and pass |

A survivor argued equivalent counts as argued only with the argument written in the report
(and, per CLAUDE.md, only when rewriting the condition was not the better fix). A survivor in
code that no test can reach without a winit window is **not** equivalent; it is escalated as a
missing seam, not argued away.

### Contract amendments
- 2026-09-28 (iteration 1, recorded for the user): the AG11 check matched ai-monitor's own
  doc comments that *state* the rule ("no `libc` here", present at HEAD too), so it failed on
  prose. It now checks non-comment lines, and adds that `libc` is not a direct dependency —
  the same intent, measured; no threshold moved.
- 2026-09-28 (user-approved, iteration 1 escalation): the Gate-3 checker found D5's
  Antigravity footer marker (`esc to cancel`) drawn under every running `agy` turn, so the
  two-of-three rule was one ordinary line from a false "waiting" — a conflict with the
  conservative-bias Constraint. The user chose "question with the first choice as its next
  non-blank row"; D5, R6.2–R6.4 are amended in the spec, and this contract's Goal now measures
  against the amended text.

## Gate 3 — Qualitative  (score + grounds + action; checker-scored)
Score each /10. Pass = every item ≥ 7 **and** no item's action names a spec violation.

- **Spec alignment** — each Decision (D1–D14) is implemented as written or as recorded in
  `## Implementation Notes`; nothing in the Non-goals was touched; the conservative bias holds
  (no path yields `Waiting` except D2/D13's finished turn, a blocking dispatch, or a dialog on
  screen, plus the two named exceptions A1 and the D7(a) hold).
- **Architecture fit** — dependency direction held; `ai-monitor` headless (no FD code) and
  policy-free (it classifies, the app decides via the unchanged `state_from`); the renderer
  agent-agnostic (it knows `Hidden` and `blocking_dialog`, never an agent); one
  `effective_state` feeds band and card; bounded idle work (G1).
- **Test strength** — tests assert outcomes (exact states, counted reads/probes, captured
  pixels), not only that code ran. GPU/font tests skip only when the resource is **absent** and
  **fail** when it is present but wrong; the golden frames skip only through `testing::headless`.
- **Naming / readability** — matches the surrounding code's idiom and comment density.

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests,
docs/comments, local renames, behaviour-preserving refactors (including extracting a pure
function so window-bound logic becomes testable).

**🛑 STOP — call a human (escalate):**
- DB schema changes
- Data-loss-capable migrations
- Auth / permission / access-control policy changes
- Payment / security-sensitive changes (secrets, crypto, billing)
- Anything conflicting with this contract's Goal or the spec — including a requirement the
  code does not meet where meeting it would change behaviour the spec did not ask for
- Adding a dependency, editing `Cargo.lock`, committing, or writing under `~/.gemini`

## Exit criteria
The loop exits only when **every** gate above passes and the Evidence Report is
emitted. Hitting a 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Each lesson is a mistake this loop already made, hardened so it can't recur.
> Phase 0 loads `status: active` lessons: `gate1-command` / `gate3-item` forms are
> folded into the gates above; `context` forms are read as guidance. Lessons are
> scoped to THIS `loop.md` — they do not propagate to other contracts. The human
> owns this section (curation); see SKILL.md "Self-Learning Lessons" + Phase 5.
>
> L1 and L2 were recorded, human-approved, in `specs/sidebar-weather-cards/loop.md`, and are
> carried into this contract at the orchestrator's request because the same gates recur.

### L1 — A mutation run counts only if its log proves it ran
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: Gate 2 counts a `cargo mutants` run only when its saved log shows `ok Unmutated baseline` and `N mutants tested`, with the test arguments written out literally — zsh does not word-split an unquoted `$VAR`, so two renderer runs (sidebar-weather-cards iterations 2–3) refused their arguments and never started, and a `grep` for MISSED/`mutants tested` printed nothing, which read as "no news".

### L2 — No test takes its expected value from the code it tests
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: No new or changed test derives its expected value by calling the function under test (it would agree with any wrong answer), and each new test is falsified — the defect it targets is applied and the test goes red — before it is trusted.
- note: the sidebar-weather-cards D17 icon check chose its expected icon through `PixelIcon::for_skin`, the mapping under test, and stayed green with Codex mapped to the terminal icon; keyed on the skin with an independent table, it fails at both scales.

### L3 — A detector marker is checked against the screens without the dialog
- cause: escalation
- source: human-approved
- form: gate3-item
- status: active
- check: Every on-screen detector marker (a line the dialog rule counts) is run against the captured screens of the same agent **without** a dialog — its busy/idle screens from the same `waiting-scan` capture — and a marker that appears on any of them is not a marker; the rule is shown to fire on none of those tails, including with one plausible line (the user's echoed reply, a question in the output) added.
- note: D5 counted Antigravity's `esc to cancel` row as a footer marker, but `agy` draws it under every running turn (the capture's `Generating...` screens), so a busy agent was one echoed `> 1. Yes, …` from a false "waiting"; the Gate-3 checker found it in the capture log that had been sitting in the scratchpad all along.

### L4 — A new gate command is run on HEAD before it gates the change
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: the AG11 check (`grep libc|proc_pid crates/ai-monitor/src`) failed on ai-monitor's own doc comments that *state* the no-libc rule, present at HEAD too — so its first red said nothing about the change; run each new Gate-1 command on HEAD first, and make a source grep skip comment lines.

### L5 — A "caught" mutant is only caught if the failing test is about it
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: window-bound `Running` mutants read as caught because an unrelated cost test (`a_non_tmux_tab_costs_exactly_the_gate_and_spawns_nothing`) failed — the new worker tests drove `Deriver::tab()` without holding `probe::serial()` and perturbed its global counters; read which test failed in the mutant's log before counting a kill, and any test that calls `tab()` holds `probe::serial()`.
