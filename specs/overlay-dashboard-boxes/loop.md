# loop.md — Loop Contract: overlay-dashboard-boxes

- pre-approved-unattended: yes

## Goal
Add the five activity sections — Tasks, Skills, Other Tools, File Activity, Agents — to the existing pure composer in `crates/app/src/dashboard.rs`, with the summary block composed last, per `specs/overlay-dashboard-boxes/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `crates/app/src/dashboard.rs` **only**
- Out: everything else — `overlay.rs`, `main.rs`, `seam.rs`, the renderer and `ai-monitor` are all out of surface. No colour (SD3), no interaction.

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| snapshot coverage | populated + fully-empty + one per section at its truncation boundary | `cargo test -p app` |
| determinism (B6c) | identical output across two separate processes with >6 tasks | rerun `cargo test -p app` |
| composed height (B1/B2) | populated `≈ 28` lines; empty `== 9`; the summary block is the **last** 4 | assertion on the composed `Vec<String>` |
| section caps (B2) | Tasks 4 · Skills 3 · Other Tools 4 · File Activity 4 · Agents 4 | fixture test driving each past its cap |
| truncation stated (B2a) | a section over cap carries `+N more` adjacent to its label, total still `104` | fixture test |
| empty states (B3) | all five reachable and inline on the rule (1 line each) | fixture test |

## Gate 3 — Qualitative
- **Heading honesty (B6/B7b)** — the sections are "Agents" and "Other Tools"; every empty state stays reachable and no heading claims coverage the store does not have.
- **Marker axes kept apart (B4)** — `✓`/`✗` is outcome, `●`/`◯` is liveness; they are never conflated, and each is a single shared constant pair.
- **Derived constants match their arithmetic (B7/B2a)** — the 85-column path budget and the `+N more` placement each follow from `RULE_COLS = 104` as written; a number that disagrees with its own derivation is a defect this contract has already seen twice.
- **Degradation ordering (B1)** — order is Tasks → Skills → Other Tools → File Activity → Agents → summary, so a 14-line panel keeps what matters.
- **Spec alignment** — R0–R8 GWTs hold.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within `dashboard.rs`, including accepting new `insta` snapshots that match the spec.
**🛑 STOP — escalate (park):**
- Touching **any** file other than `crates/app/src/dashboard.rs`.
- Changing `ai-monitor`'s ring caps or any fold behaviour to suit the layout — caps and display policy are app-side (SD4); the crate's caps are not this feature's to move.
- Adding a pending-tool marker — the pending map is deliberately excluded from `TelemetrySnapshot`, so that data does not reach this layer.
- Drawing agent tree connectors (`├─`/`└─`) — there is no parent linkage in the data (D9).
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Appended by the unattended run of 2026-07-28. `source: auto-unattended` — no
     human approved these at record time; review and prune. -->

### L1 — bound the row, not just the field you happen to be truncating
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: For every composed row, drive EVERY string that reaches it from raw telemetry to 300 characters and every duration to `u64::MAX`, and assert the row is still `<= RULE_COLS`.
- note: the first draft truncated only the *variable* segment of a row. A task's `status` and an agent's `subagent_type`/`model` are raw passthrough strings, so a 150-column status left the variable part zero-width AND still overran the rule; separately, the file row bypassed `fit_row` entirely and could reach 105 columns because `dur` emits 9 (`70000h00m`) where the derivation reserves 8. Both found by the Gate 3 checker, neither by the fixture-based tests — a fixture only ever proves the fixture.

### L2 — `insta` stops a test at its FIRST mismatch
- cause: gate-fail
- source: auto-unattended
- form: gate1-command
- status: active
- check: One `assert_snapshot!` per `#[test]`. Never several in one test.
- note: five section snapshots in one test wrote only the first `.snap.new` and hid the other four, so accepting looked complete after one round and the next run failed again. It also means a later section's regression is invisible until the earlier one is fixed.

### L3 — the root is an ancestor
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: —
- note: `short_path("/etc/hosts")` returned `etc/hosts` — a *relative* path naming a different file — because the leading empty segment is filtered before counting ancestors. Shortening a path must preserve whether it was absolute.

### L4 — residual: budgets are character counts, the renderer clips display columns
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: every budget here (`RULE_COLS`, `PATH_COLS`, `fit_row`) counts `chars()`, while `overlay_layer.rs` clips on display columns, where CJK spends 2. A 100-character Hangul subject passes `no_row_overruns_the_rule` and still visibly outruns its own rule. Inherent to the width-blind design and clipped safely by the renderer, but it belongs in the spec's Known Gaps — a spec edit is outside this loop's declared surface, so it is recorded here for the human.
