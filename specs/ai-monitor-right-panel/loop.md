# loop.md — Loop Contract: ai-monitor-right-panel

## Goal
**Verify** the already-complete implementation of `specs/ai-monitor-right-panel/spec.md` — the
ai-monitor's move from a full-width bottom overlay to a right-docked, drag-resizable panel that
reserves terminal columns. This is a verification run: Phase 1 WORK is a no-op on iteration 1,
so VERIFY is effectively the first action.

## Scope & non-goals
- **In:** the spec's Declared Surface — `crates/renderer/{lib,monitor_model,monitor_layout,monitor_panel}.rs`,
  `crates/app/{dashboard,overlay,input,terminal_app,settings,seam}.rs`,
  `crates/ai-monitor/src/telemetry.rs`, `crates/common/src/config.rs`.
- **Out:** `crates/term-core/**` — non-destructive reflow is `specs/grid-reflow/spec.md` (D22).
  Touching it is a boundary violation, not a fix.

---

## Gate 1 — Pass/Fail  (binary, 100% required)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| term-core untouched (R0.4/D22) | `sh specs/ai-monitor-right-panel/check-no-term-core.sh` |
| deleted mechanism absent (R3.3) | `! grep -nE '^[^/]*\b(RULE_COLS\|fn fit_row\|fn rule_with\|fn rule_empty\|fn compose)\b' crates/app/src/dashboard.rs` |
| retired layer absent (D14) | `! test -e crates/renderer/src/overlay_layer.rs` |
| no GUI crates (C2) | `! grep -nE '^(egui\|iced\|druid\|slint\|gtk\|gpui\|dioxus\|tao)\b' crates/*/Cargo.toml` |
| dependency direction (C4) | `! grep -nE '^(wgpu\|winit)\b' crates/term-core/Cargo.toml` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| suites green | `== 18` | count of `test result: ok` |
| sub-requirements traced | `== 63` | each R*.* maps to a test, or to a limit explicitly recorded in the spec |
| sub-requirements *unevidenced* | `== 0` | traced but with neither a test nor a recorded limit |
| overstated limits | `== 0` | a limit the spec/report claims is covered which is not |

## Gate 3 — Qualitative  (checker subagent — maker ≠ checker)

- **Spec alignment** — do the 63 GWTs actually hold in the code, or only in test *names*?
  A test whose name matches a requirement but whose body asserts something weaker is a miss.
- **Test genuineness** — are the assertions mutation-resistant? Several were mutation-proved
  during the build; sample the ones that were not and judge whether they would survive the
  obvious wrong implementation.
- **Honesty of recorded limits** — R0.4 (inherited), R6.6 (no e2e), R10.1 (inspection),
  R7.4 (source-level), the backfill-offset call site, and the missing GUI eyeball. Each must be
  stated accurately. **An overstated limit is a finding; an accurately stated one is not.**
- **Architecture fit** — renderer owns pixel layout, app owns policy; `dashboard.rs` is pure;
  `term-core` untouched; the panel's shaping cache is real, not nominal.

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** missing tests for code already in scope; doc/comment
corrections; lint/format; local renames; behaviour-preserving refactors that keep Gate 1 green;
correcting an overstated limit in the spec's own prose.

**🛑 STOP — escalate:**
- Any edit under `crates/term-core/**` (D22/R0.4 — that is a different feature).
- Changing a *decision* (D1–D22) rather than the code that implements it.
- Weakening or deleting an existing assertion to make a gate pass.
- Anything conflicting with the approved spec.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.

## Lessons

### L1 — never `git checkout --` an uncommitted working tree
- cause: escalation
- source: human-approved
- form: gate1-command
- status: active
- check: `test -z "$(git status --porcelain -- crates/ | head -1)" || git stash list >/dev/null`
- note: iteration 1 mutation-tested a gate script, then reverted with `git checkout -- <file>`
  on a tree where the WHOLE feature was uncommitted — reverting to HEAD wiped ~1400 lines of
  T5/T6 work. Recovered only because the subagent transcript held the Write+Edit sequence.
  Mutate a scratchpad copy, or commit first. Never `checkout --` to undo a test mutation.

### L2 — a name-absence grep must scan production code only
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `sh specs/ai-monitor-right-panel/check-deleted-mechanism.sh`
- note: the first R3.3 gate grepped the whole file and matched the string literals inside the
  test that asserts those names are absent — it failed precisely when the requirement held.
  The check must cut at `#[cfg(test)]` and fail loudly if that boundary is missing.

### L3 — commit before verifying
- cause: escalation
- source: human-approved
- form: context
- status: active
- check: —
- note: a verification run that can modify files must start from a committed baseline, so any
  destructive step is recoverable by `git restore` rather than by transcript archaeology.

### L4 — a terminated subagent leaves its scaffolding behind
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `test -z "$(git status --porcelain --untracked-files=all -- crates/ | grep -v '^ M')"`
- note: a Gate-3 checker killed by a session limit left `crates/renderer/tests/zz_gate3_probe.rs`
  behind — its own header said "deleted before the check ends", but its cleanup never ran. The
  stray integration test broke `cargo test` and `cargo fmt --check` on an otherwise green tree,
  and the failure looked like a code regression. After any subagent run, check `git status` for
  untracked leftovers before trusting a gate result.
