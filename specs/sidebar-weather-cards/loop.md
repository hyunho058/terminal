# loop.md — Loop Contract

> The supervisor document for `sidebar-weather-cards` (post-execution verification gate).
> Defines what "done" means for the T1–T8 implementation so the loop verifies it instead
> of trusting it.

## Goal
The uncommitted implementation on `feat/improve-card-ui` satisfies every sub-requirement
(R0.1–R18.3, 75 in all — R11.5 added with D11's amendment) of `specs/sidebar-weather-cards/spec.md`, and violates none of its
Decisions, Constraints or Non-goals.

## Scope & non-goals
- In: the files T1–T8 touched (`crates/term-core/src/{grid,parser,lib}.rs`;
  `crates/renderer/src/{sidebar_theme,sidebar_icons,sidebar_text,sidebar_card,sidebar_row,
  sidebar_layout,sidebar_shapes,tab_bar,lib,monitor_panel,preedit_layer}.rs`;
  `crates/renderer/tests/golden_frame.rs`; `crates/app/src/{sidebar_model,waiting,session,
  tabs,terminal_app}.rs`) and tests for code already in them.
- Out: the sidebar header and search pill, line-1 precedence, the ai-monitor panel, new
  skins (the spec's Non-goals); `Cargo.lock` and every `Cargo.toml`; commits; the
  `/run-term` eyeball check (a human Post-work step).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust: `cargo build` is the type check |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |
| GPU tests really ran (R16.6) | `test -z "$SKIP_GPU_TESTS"` — with it unset, `testing::headless` fails rather than skips, so a green golden frame means it rendered |
| term-core stays GPU/window-free (C4) | `! cargo tree -p term-core -e normal --prefix none \| grep -qE '^(wgpu\|winit) '` |
| no dependency added, lockfile untouched | `git diff --quiet HEAD -- Cargo.lock Cargo.toml 'crates/*/Cargo.toml'` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| unargued mutation survivors, term-core diff | `= 0` | `cargo mutants --in-diff <term-core diff> --cargo-test-arg -p --cargo-test-arg term-core` |
| unargued mutation survivors, renderer diff | `= 0` | `cargo mutants --in-diff <renderer diff>` with `-p renderer --lib --test golden_frame` |
| unargued mutation survivors, new files | `= 0` | `cargo mutants -f sidebar_card.rs -f sidebar_text.rs`, same test args |
| unargued mutation survivors, app diff | `= 0` | `cargo mutants --in-diff <app diff>` with `-p app --lib` |
| sub-requirements with a named, passing verifier | `75 / 75` | traceability matrix: each R-id → the test (or Gate-1 command) that checks it, each test confirmed to exist and pass |

A survivor argued equivalent counts as argued only with the argument written in the report
(and, per CLAUDE.md, only when rewriting the condition was not the better fix).

## Gate 3 — Qualitative  (score + grounds + action; checker-scored)
Score each /10. Pass = every item ≥ 7 **and** no item's action names a spec violation.

- **Spec alignment** — each Decision (D1–D21) is implemented as written; nothing in the
  Non-goals was touched.
- **Architecture fit** — dependency direction held; `build_card` pure (D16); the app resolves
  status and the renderer only draws it (D7); one geometry formula for draw and hit-test.
- **Test strength** — tests assert outcomes (exact values, captured pixels), not only that
  code ran. A GPU/font unit test may skip only when the resource is **absent**
  (`crate::headless()` → `None`, `FontSet::load()` fails) and must **fail** when it is
  present but wrong; the golden frames skip only through `testing::headless` (R16.6).
- **Naming / readability** — matches the surrounding code's idiom and comment density.

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

### Contract amendments
- 2026-09-27 (user-approved, iteration 1 escalation): **Test strength** was "GPU tests skip
  only through `testing::headless`". That is stricter than the crate's documented unit-test
  policy (`crate::headless`: adapterless → skip) and the user's standing convention (skip
  when absent, fail when present but wrong); it now states that convention. The golden
  frames keep the strict policy (R16.6).
- 2026-09-27 (user-approved): D11 amended in the spec (the `≡` slot is the widest icon as
  drawn), adding R11.5 — the traceability bar is now 75/75. The first count, 75, was a
  typo for the spec's then-74 headings.
- 2026-09-27 (user-approved, iteration 3 escalation): D2 amended in the spec — one text
  draw per atlas on the normal path; a failed card atlas's fallback text (R2.6) is a
  second grid-atlas draw, submitted under the popup.
- 2026-09-27 (user-approved, iteration 4): D1's wording amended in the spec — card
  geometry is scaled once through `CardMetrics::scaled`, beside `SidebarMetrics::scaled`.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests,
docs/comments, local renames, behaviour-preserving refactors.

**🛑 STOP — call a human (escalate):**
- DB schema changes
- Data-loss-capable migrations
- Auth / permission / access-control policy changes
- Payment / security-sensitive changes (secrets, crypto, billing)
- Anything conflicting with this contract's Goal or the spec — including a requirement the
  code does not meet where meeting it would change behaviour the spec did not ask for
- Adding a dependency, editing `Cargo.lock`, or committing

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

<!-- Append entries below. A fresh contract has none. -->

### L1 — A mutation run counts only if its log proves it ran
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: Gate 2 counts a `cargo mutants` run only when its saved log shows `ok Unmutated baseline` and `N mutants tested`, with the test arguments written out literally — zsh does not word-split an unquoted `$VAR`, so two renderer runs (iterations 2–3) refused their arguments and never started, and a `grep` for MISSED/`mutants tested` printed nothing, which read as "no news".

### L2 — No test takes its expected value from the code it tests
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: No new or changed test derives its expected value by calling the function under test (it would agree with any wrong answer), and each new test is falsified — the defect it targets is applied and the test goes red — before it is trusted.
- note: the D17 icon check (iteration 5) chose its expected icon through `PixelIcon::for_skin`, the mapping under test, and stayed green with Codex mapped to the terminal icon; keyed on the skin with an independent table, it fails at both scales.
