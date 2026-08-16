# loop.md — Loop Contract

> Supervisor document for **tabsession-pane-cwd-gate**
> (`specs/tabsession-pane-cwd-gate/spec.md`).
> This is a **post-implementation verification** contract: all six tasks (T1–T6) are already
> built. The loop starts by **verifying**, not by re-doing the work — iteration 1's WORK phase
> is a no-op.
>
> Base commit for every diff-based check: **`0b64fdd`** (was `7b7c0a2` before the rebase
> onto `main`, which picked up #31 `card-live-cwd`).

## Goal
Prove the shipped implementation satisfies the spec's `## Requirements` (R0–R10, 49 GWT
sub-requirements) without violating its `## Constraints` or the project rules
(`dependency-direction`, `no-gui-crates`, `macos-only`, `zsh-local-only`) — and in particular
that Confirmed Goal 3 still holds **by a stated argument**: no path may confirm another
application's log with `owned: true`.

## Scope & non-goals
- **In:** the six files in the spec's `## Declared Surface` plus the three recorded deviations
  below; test additions for behaviour already in scope; comment/doc corrections; lint/format
  fixes.
- **Out:** new features beyond R0–R10; changing the approved decisions D1–D12; any edit under
  `crates/ai-monitor/` (R10.2 forbids it outright); anything that would require re-approving the
  spec.
- **Recorded deviations from the Declared Surface** — the checker must rule on whether each is
  the smaller evil, not merely note it:
  1. `crates/app/src/tabs.rs` — adds `Tabs::active_agent_pid(&TabMetaSnapshot)`, the accessor
     that reads the new `TabMeta::agent_pid` for the widened seam. The spec's Declared Surface
     names `terminal_app.rs`'s call sites but not the accessor they call.
  2. `docs/architecture.md` — a `G5` known-gap entry for D11's card/panel directory divergence
     plus assumptions A1/A2. This is R9.1's "documented in Known Gaps"; the spec did not name a
     file for it.
  3. `crates/app/src/sidebar_model.rs` — **forced by a rebase, not chosen.** `card-live-cwd`
     landed on `main` first as #31 (the spec's Declared Surface predicted the opposite order),
     adding a `meta_at()` test helper that constructs a `TabMeta` literal. That literal cannot
     compile against the widened struct without the new field, so the rebase requires
     `agent_pid: None` there. One line plus its comment, in a test helper, no production
     behaviour. The alternative — giving `TabMeta` a `..Default::default()` escape at that site —
     was rejected as the larger change: it would make future field additions *stop* being visible
     edits at that call site, which is the property `tabmeta_still_has_exactly_its_five_published_fields`
     exists to protect. The base commit for every diff-based check therefore moves from `7b7c0a2`
     to `0b64fdd`.
- **Recorded judgement calls** — same treatment; each is a candidate spec conflict (🛑) if the
  checker finds it exceeds the approved design:
  1. **Known Gap closed rather than left to L4.** The spec records "how `open()` reaches the
     published pid on two of its three callers is unspecified … left to L4", and D5 proposed
     `open()` seed `last_seen_pid`. The implementation instead has `on_active_changed` maintain
     `last_seen_pid` on every `Some` publish, so all three `open()` callers see a current value
     without touching `AppCtx` or `OverlayCommand::RestoreMonitor`. D10's early-out remains the
     declared backstop.
  2. **`rivals_known = false` forced on D7 row 3 as well as row 4.** The spec's table states it
     only for row 4. Row 3 is mixed-basis by the same argument and suppresses tier 2 anyway, so
     this only ever *withholds* the uniqueness rescue — but it is beyond what the table says.
- **Explicitly out of automated reach** — these must NOT be marked passed by this loop:
  - **R0.1** (the reported configuration resolves) and **R0.4** (`Resolved.owned == true` for
    it) — needs a live tmux client, a real pane running `claude`, and a real log being written.
    Requires `/run-term` on the reporting machine.
    **[PASSED 2026-08-16, by human observation — not by this loop.]** Fixture
    `pane-cwd-gate-test`, pane `17525` → `claude` `17591`, session
    `9bb64bfb-9722-4ec1-8c4d-3134e86e1073`, kernel and registry cwd agreeing (D7 row 2). The
    operator reported `CONNECTED` with the session id, where the same shape previously read
    `NO SESSION`. Recorded in `spec.md` § Post-work. This does **not** move the Gate-2 coverage
    count, which measures *automated* evidence only.
  - **R6.7** (quit `claude`, `cd`, start a new one → the panel re-points within one tick) —
    needs a live process tree and a running app.
    **[PASSED 2026-08-16, by human observation — not by this loop.]** Same pane `47553`, its
    `claude` replaced `47594` → `48112`, session `a44ec234…` → `cd132e29…`, and the panel followed
    with no tab switch. The operator restarted in place, so the **directory was unchanged** across
    the re-point — which makes it a sharper D5 × D10 check than the scripted variant: only the pid
    differed, so a `reopen_is_inert` keyed on the directory alone would have swallowed it.
    R6.7's "and directory" half remains unexercised live (covered headlessly by R3.1–R3.5).
    Recorded in `spec.md` § Post-work.
  All three are now **passed by human observation**, and all three remain excluded **by name**
  from the Gate-2 coverage count regardless: the metric is about what the suite can prove unattended, not about what a person saw
  once.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| format | `cargo fmt --all -- --check` |
| typecheck | N/A — `cargo build` is the type check for Rust (no separate command) |
| **R10.2** `crates/ai-monitor` unmodified | `test -z "$(git diff --name-only 0b64fdd -- 'crates/ai-monitor/')"` — the spec forbids touching the resolver crate at all; D3's tier-2 suppression is expressed purely by leaving `Wanted::since_ms` as `None`. Validated in both directions (fires when any file under that path is staged) |
| **R2.4** `choose_claude` is reader-free | `grep -q 'fn choose_claude(root: u32, shell_pid: u32, find: impl Fn(u32) -> Option<u32>) -> Option<u32>' crates/app/src/tabsession.rs` — the cwd reader must be **gone from the signature**, not merely unused. A left-behind parameter is how a gate gets quietly re-introduced |
| **Declared surface** respected | `test -z "$(git diff --name-only 0b64fdd -- '*.rs' \| grep -vE 'crates/app/src/(tabsession\|overlay\|seam\|terminal_app\|tabs\|sidebar_model)\.rs\|crates/app/src/tabmeta/(mod\|worker)\.rs')"` — the six declared files plus deviations 1 and 3. **Validated in both directions**: dropping `tabs` from the alternation makes it report `crates/app/src/tabs.rs`, so it can genuinely fail. `sidebar_model` was added to the allow-list **only** after the rebase forced the edit (deviation 3); it was not in the originally approved list |
| **D6 trigger is live** | `awk '/fn feed_agent_states/,/^    }$/' crates/app/src/terminal_app.rs \| grep -vE '^[[:space:]]*(//\|///\|\*)' \| grep -q 'active_agent_pid'` — the published pid must actually reach the seam. `agent_pid_moved` can be perfectly correct and the feature still dead if nothing feeds it; **a sibling feature in this repo shipped exactly that bug**. The comment filter is load-bearing: without it a commented-out call still matches. Validated: correctly silent over `push_agent_states` |
| **R9.1** the divergence is recorded where a reader will find it | `grep -q 'G5' docs/architecture.md && grep -qi 'card' docs/architecture.md` — D11's card-vs-panel directory divergence must be documented as a known gap, not left only in the spec |
| **`macos-only`** no new `#[cfg(target_os)]` | `test "$(git diff 0b64fdd -- '*.rs' \| grep -c '^+.*cfg(target_os')" = 0` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| tests passing | `≥ 1251`, `0 failed` | `cargo test --workspace` — sum the `test result:` lines ONLY, never the `running N tests` lines (they double-count). 1230 was the measured value at contract time; the base moved to `0b64fdd` (#31 added 18 tests of its own), remeasured at 1251 |
| clippy warnings | `= 0` | exit code of the `-D warnings` command |
| rustfmt drift | `= 0 files` | `cargo fmt --all -- --check` |
| sub-requirements with automated evidence | **`≥ 44 of 46`** | manual trace of each GWT to a named test or Gate-1 command. **Baseline measured at contract time is 41.** 3 of the 49 are excluded **by name**: R0.1, R0.4, R6.7 (see Scope). The gap to 44 is three specific items, all missing tests for in-scope behaviour and therefore Auto-fix: **R1.4** (a tab with no `claude` yields no hint *from `hint_for_with`*, not just from `choose_claude`), **R4.3** (`refresh_rivals` publishes a real `RivalSet` from a `Some` hint), and **R3.3's warn emission** (the `tracing::warn!` naming the pid and both directories — currently only the *transition rule* is tested, not that anything emits). **R3.8** may remain code-traceable: it is a property of `ai_monitor`'s tier-1 exact-filename lookup, which R10.2 forbids this feature from touching |
| files edited outside Declared Surface | `= 0` beyond the 3 recorded deviations | the Gate-1 declared-surface command; `tabs.rs` is the single allowed extra `.rs` file, `docs/architecture.md` the single allowed non-`.rs` |
| new GUI-toolkit deps | `= 0` | `git diff 0b64fdd -- '*/Cargo.toml'` shows no egui/iced/druid/slint/gtk/gpui/dioxus/tao (`no-gui-crates`) |
| new dependencies of any kind | `= 0` | the spec's Constraints say "No new dependencies"; `git diff 0b64fdd -- '*/Cargo.toml' Cargo.lock` |
| `term-core` GPU/window deps | `= 0` | `grep -nE '^[[:space:]]*(wgpu\|winit)' crates/term-core/Cargo.toml` empty **and** `grep -rHnE '^[[:space:]]*use (wgpu\|winit)' crates/term-core/src/` empty (`dependency-direction`). **Not** a bare `grep -rn "wgpu\|winit"`: that file's doc comments *assert the absence of* these deps and the naive form always fires |
| unbounded tmux waits on the event-loop thread | `= 0` | every UI-thread resolve routes through `tabsession::resolved_root`'s `recv_timeout(250ms)`, never `effective_root_uncached` directly (R10.1). D4 should have **reduced** the tmux tab's open path from two such waits to one — verify the reduction, not just the bound |

## Gate 3 — Qualitative  (score + grounds + action)

Scored by a **separate checker subagent** (maker ≠ checker), given only the diff, this contract,
and the spec — never the maker's reasoning.

- **Spec alignment — the five-row chain.** Does `resolve_hint_cwd` implement D7's table
  faithfully, row for row, including which *directory* each row uses and which rows are
  mixed-basis? Is tier 2 genuinely suppressed on `Disputed` and on row 5, at the point `Wanted`
  is built rather than by blanking `SessionHint::started_ms` (D3 says the two are easy to
  confuse and names the second as breaking rotation detection *and* the rival-refresh guard)?
- **Confirmed Goal 3, by argument.** Can any path now confirm another application's log with
  `owned: true`? Trace it, do not assume: the resolved cwd must feed `rival_starts_checked` so
  the veto is populated on the same basis as the directory (D7's "the ordering is load-bearing"),
  and no path may yield a falsely `known: true` empty rival set.
- **Guard durability — are the pins real or vacuous?** D4's no-capture guard, D10's four
  early-out conditions, D5's `Some`-guarded compare, and R8.2's replacement for the assertion
  that became vacuous. For each: could it pass against a broken implementation? This repo has
  shipped an "apparent coverage" test before (`tmux.rs:648-651`) and the spec cites it.
- **Test-retarget integrity (R8.1/R8.2).** Were the five gate tests genuinely re-targeted to the
  properties that replaced the gate, or quietly weakened? Two were deleted as duplicates — is
  each deletion's recorded reason accurate, and is the surviving test actually equivalent?
- **Deviation and judgement-call honesty.** The two Declared-Surface deviations and the two
  judgement calls listed in Scope: is each the smaller evil, and recorded where a reviewer would
  find it? Does judgement call 1 (closing a Known Gap the spec left to L4) or 2 (`rivals_known`
  on row 3) **conflict with the approved spec**? If either does, that is a 🛑 boundary, not a
  score.
- **Naming / readability at the call site.** `HintCwd`, `hint_or_pane_cwd`, `reopen_is_inert`,
  `agent_pid_moved` — does each read correctly where it is used, and does the code carry its
  invariant the way this module's existing fields do?

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact, a comparison>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests (specifically the
three Gate-2 coverage gaps named above), docs/comments, local renames, behaviour-preserving
refactors that keep every Gate-1 command green.

**🛑 STOP — call a human (escalate):**
- DB schema changes · data-loss-capable migrations · auth/permission changes ·
  payment/security-sensitive changes
- **Any edit under `crates/ai-monitor/`** — R10.2 forbids it, so a fix that seems to need one is
  by definition a spec conflict
- **Re-introducing a cwd comparison** in `choose_claude`, or weakening `resolve_owned_session`'s
  strictness — both are decisions D2 and the Constraints settled
- Anything conflicting with this contract's Goal or with the spec's decisions D1–D12, including
  a checker finding that judgement call 1 or 2 exceeds the approved design
- Marking R0.1, R0.4 or R6.7 as passed — they are out of automated reach by name

## Exit criteria
The loop exits only when **every** gate above passes and the Evidence Report is emitted, with
R0.1 / R0.4 / R6.7 reported as **outstanding manual verification**, never as passed. Hitting a
🛑 boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Each lesson is a mistake this loop already made, hardened so it can't recur.
> Phase 0 loads `status: active` lessons: `gate1-command` / `gate3-item` forms are
> folded into the gates above; `context` forms are read as guidance. Lessons are
> scoped to THIS `loop.md` — they do not propagate to other contracts. The human
> owns this section (curation); see SKILL.md "Self-Learning Lessons" + Phase 5.

<!-- Append entries below. A fresh contract has none. -->

### L1 — bulk edits bypass the rustfmt hook
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: the PostToolUse rustfmt hook fires only on Edit/Write, so edits applied through the shell (`python3`, `sed`) land unformatted and fail Gate 1's `cargo fmt --all -- --check`; run `cargo fmt --all` after any such edit rather than at the end of the run.

### L2 — a rule inside a spawned closure has no test seam
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Is every new rule or mapping placed where a test can reach it? Logic inside a `std::thread::spawn` closure, a callback, or a method needing a live overlay is unreachable from a unit test — extract it to a named free function, as this module already does for `Repoint`, `pane_cwd_with`, `watch_with_rivals` and `rival_set_of`.
- note: the hint→`RivalSet` mapping lived inside `refresh_rivals`' worker closure, so R4.3 — the whole recovery this feature delivers for the second `hint_for` caller — had zero coverage until it was extracted to `rival_set_of`.

### L3 — a source-text pin must match code, not prose about the code
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Does each source-text pin's needle match the CODE rather than a comment or message literal describing it? Assert the full field spelling (`kernel = %kernel.display()`), never the bare identifier, and anchor any extracted region with a positive assertion before asserting a negative over it.
- note: `warn.contains("kernel")` was also satisfied by the warning's own message text ("disagrees with the kernel's"), so deleting the structured field would have left the pin green — the same apparent-coverage failure R8.2 exists to prevent, one level down.
