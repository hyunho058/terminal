# loop.md — Loop Contract: ai-monitor-idle-timestamps

## Goal
**Verify** the already-complete implementation of `specs/ai-monitor-idle-timestamps/spec.md` —
the panel rebuilding its model on wakes it already pays for (Option 1 / D2), and a single
bounded deadline so a quiet session's pill stops asserting CONNECTED (Option 3 / D3).
This is a verification run: Phase 1 WORK is a no-op on iteration 1, so VERIFY is the first
action.

## Scope & non-goals
- **In:** `crates/app/src/{dashboard,overlay,terminal_app,seam,tabs,lib}.rs`,
  `crates/app/src/tabmeta/{mod,worker}.rs`, `crates/app/tests/wiring.rs`, and the three
  amendment sites (`specs/ai-monitor-right-panel/spec.md`, the `publish_if_changed` doc
  comment, R7.4's part-2 comment).
- **Out:** `crates/term-core/**` and `crates/ai-monitor/**` — the Constraints put both out of
  scope; this is an app-side display concern. Touching either is a boundary violation, not a fix.
- **Out:** the pill's vocabulary and `PanelStatus` (D4), and `IDLE_AFTER_MS`'s value (D10).

---

## Gate 1 — Pass/Fail  (binary, 100% required)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| committed baseline (see L1) | `test -z "$(git status --porcelain -- crates/ specs/ai-monitor-right-panel/)"` |
| no stray untracked files (see L2) | `test -z "$(git status --porcelain --untracked-files=all -- crates/ \| grep -v '^ M')"` |
| term-core untouched (Constraints) | `test 0 -eq "$(git diff --name-only HEAD~1 -- crates/term-core \| wc -l)"` |
| ai-monitor untouched (Constraints) | `test 0 -eq "$(git diff --name-only HEAD~1 -- crates/ai-monitor \| wc -l)"` |
| `IDLE_AFTER_MS` stays private (R4.2/D15) | `! grep -qE 'pub(\(crate\))? +const +IDLE_AFTER_MS' crates/app/src/dashboard.rs` |
| overlay never names the threshold (R4.2) | `cargo test -p app --lib -- r4_2_the_overlay_never_names_the_threshold` |
| R7.4 ban list intact + passing (R7.4/D12) | `cargo test -p app --lib -- r7_4_the_loop_still_waits_and_the_panel_schedules_no_wake` |
| R7.4 ban list unedited (D12 negative deliverable) | `test 0 -eq "$(git diff HEAD~1 -- crates/app/src/terminal_app.rs \| grep -cE '^[-+].*\"(Duration\|Instant\|thread::sleep\|recv_timeout\|set_timeout\|ControlFlow::Poll\|ControlFlow::WaitUntil)\"')"` |
| no new dependency (Constraints) | `test 0 -eq "$(git diff --name-only HEAD~1 -- 'crates/*/Cargo.toml' Cargo.toml \| wc -l)"` |
| no `cfg(target_os)` branch (macos-only) | `test 0 -eq "$(git diff HEAD~1 -- 'crates/**/*.rs' \| grep -c '^+.*cfg(target_os')"` |
| no `unwrap()` added to production (CLAUDE.md) | `test 0 -eq "$(git diff HEAD~1 -- crates/app/src/ \| grep '^+' \| grep -c '\.unwrap()')"` |
| dependency direction (C4) | `! grep -qE '^(wgpu\|winit)' crates/term-core/Cargo.toml` |
| RP-D2 cites the live G1 guarantee (R7.1) | `sh specs/ai-monitor-idle-timestamps/check-rp-d2-citation.sh` |
| the three amendment sites are written (R7.1–R7.3, R8.2) | `sh specs/ai-monitor-idle-timestamps/check-amendment-sites.sh` |
| every changed function is mutation-scoped (see L5) | `sh specs/ai-monitor-idle-timestamps/check-mutants-scope.sh` |

> The `HEAD~1` comparisons assume the feature is committed as one commit (L1). If it is
> committed as several, widen the range to the feature's merge-base rather than relaxing
> the check.

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| app-crate lib tests | `>= 769` | `cargo test -p app --lib` |
| sub-requirements traced | `== 36` | each `#### R*.*` maps to a named test, or to a limit explicitly recorded in the spec |
| sub-requirements *unevidenced* | `== 0` | traced but with neither a test nor a recorded limit |
| overstated limits | `== 0` | a limit the spec or the report claims is covered which is not |
| mutation survivors on changed functions | `== 0` | `cargo mutants -j 4 -f <file> -F <changed fns>` over dashboard/worker/overlay/tabs/seam/lib |

## Gate 3 — Qualitative  (checker subagent — maker ≠ checker)

- **Spec alignment** — do the 36 GWTs actually hold in the code, or only in test *names*?
  A test whose name matches a requirement but whose body asserts something weaker is a miss.
  Scrutinise R0.2 especially: it is the feature's headline property and cannot be fully
  verified headlessly.
- **The feature is not inert** — the spec's two most dangerous findings were an inert feature
  (D15's off-by-one) and a truthfulness inversion (D18). Judge whether the shipped arming rule
  actually fires on the *primary* path: a clean turn ending, where the timestamp arrives on the
  watcher and the state arrives 500 ms later on the worker's publish.
- **Honesty of recorded limits** — R0.2's headless substitute, the `show_agent_status` gap, the
  transient-walk-failure gap, and the LIVE-card badge finding. Each must be stated accurately.
  **An overstated limit is a finding; an accurately stated one is not.**
- **Architecture fit** — `dashboard.rs` stays pure and behaviour-unchanged; the overlay learns
  no threshold and holds no clock; the worker owns the only timer; the seam widening is
  justified and does not smuggle an `AppRequest` route.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** missing tests for code already in scope; doc/comment
corrections; lint/format; local renames; behaviour-preserving refactors that keep Gate 1 green;
correcting an overstated limit in the spec's own prose.

**🛑 STOP — escalate:**
- Any edit under `crates/term-core/**` or `crates/ai-monitor/**` (Constraints).
- Changing a *decision* (D1–D18, or `RP-D2`/`RP-D9`/`RP-D20`) rather than the code implementing it.
- Changing `IDLE_AFTER_MS`'s value, or `PanelStatus`'s variants/labels/colours (D4/D10).
- Editing R7.4's ban list (D12 — its central claim is that no edit is needed).
- Weakening or deleting an existing assertion to make a gate pass.
- Fixing the LIVE-card badge finding by narrowing `rung_key` — that reopens `RP-D19`'s
  keys-on-appearance rule, which this spec does not own.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.

## Lessons

### L1 — commit before a verification run that can auto-fix
- cause: escalation
- source: human-approved
- form: gate1-command
- status: active
- check: `test -z "$(git status --porcelain -- crates/ specs/ai-monitor-right-panel/)"`
- note: adopted from `specs/ai-monitor-right-panel/loop.md` L1/L3 because this run began in
  exactly the state that lesson was written about — 1520 uncommitted lines across 10 files.
  A loop that may edit files must start from a committed baseline so any destructive step is
  recoverable with `git restore` rather than transcript archaeology.

### L2 — a terminated subagent leaves its scaffolding behind
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `test -z "$(git status --porcelain --untracked-files=all -- crates/ | grep -v '^ M')"`
- note: adopted from the sibling contract's L4. A Gate-3 checker killed mid-run once left a
  stray integration test behind, breaking `cargo test` and `cargo fmt --check` on an otherwise
  green tree, and the failure looked like a code regression. Check `git status` for untracked
  leftovers after any subagent run before trusting a gate result.

### L4 — an absence grep over PROSE cannot tell a claim from its retraction
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: three Gate-1 failures in this run were all the same authoring mistake, and none was a
  real violation. The `cfg(target_os)` scan matched the spec's own Constraints line AND its own
  definition inside this file; RP-D2's citation check matched the amendment note that records
  the old citation as dead; the "free refresh" check matched the sentence disavowing the phrase.
  Over prose, presence of a word is not evidence of a claim — a correction necessarily quotes
  what it corrects. Prefer a POSITIVE check for the phrase that carries the required meaning,
  and when only a negative will do, scope it to the region the requirement is about (an isolated
  rationale bullet, a function body) rather than a whole file. Tuning such a regex until it
  agrees with you is how a gate ends up protecting nothing; the third one was deleted, not
  repaired.

### L5 — scope mutation testing by the functions the DIFF changed, not by the ones you wrote
- cause: gate-fail
- source: auto-unattended
- form: gate1-command
- status: active
- check: `sh specs/ai-monitor-idle-timestamps/check-mutants-scope.sh`
- note: the run scoped `cargo mutants -F` to the functions it authored, which excluded
  `tabmeta::worker::run` — changed by this feature (a new local, a new `Cmd` arm, and the wake
  block) but not written by it. `replace run with ()` survived, meaning the two lines that turn
  an armed deadline into a wake could be deleted with all 769 tests green and the feature wholly
  inert: every observable still reported a decision while the thing decided never happened, the
  failure class `golden_frame.rs` exists for. The run then claimed "0 survivors on every changed
  function", which was false in the artefact a future reader trusts most. Found by the Gate-3
  checker, not by the maker — which is the argument for maker ≠ checker in one example.

### L3 — a name-absence grep must scan production code only
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: adopted from the sibling contract's L2. This feature's own `r4_2` scan already honours
  it — it strips trailing comments as well as whole-line ones, which the older R7.4 scan does
  not — but any NEW absence check added here must cut at `#[cfg(test)]` or it will match the
  string literals inside the very test asserting the absence.
