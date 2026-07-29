# loop.md — Loop Contract: telemetry-model

- pre-approved-unattended: yes

## Goal
Add a stateful, bounded `SessionTelemetry` to `crates/ai-monitor` whose `apply(&mut self, line: &str)` folds one Claude Code JSONL line into token totals, event times, tool/skill/file/subagent rings and a keyed tasks map — per `specs/telemetry-model/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `crates/ai-monitor/src/telemetry.rs`, `crates/ai-monitor/src/lib.rs`, `crates/ai-monitor/tests/**`, `crates/ai-monitor/Cargo.toml`
- Out: everything else. No presentation policy of any kind (SD4). No file I/O — this feature folds strings.

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
| usage dedupe (D1) | 4 replica lines sharing `(message.id, requestId)` fold **once**, not 4× | fixture test asserting `total_output` |
| retry admitted (D1) | same `message.id` under 2 `requestId`s folds **twice** | fixture test |
| bounded state (D6) | pending map ≤ 64, dedupe set ≤ 64, rings ≤ 32/16/32/32, tasks map ≤ 64 | tests driving past each cap |
| malformed-safe (D10) | 3 bad-line cases pass, `parse_errors` counts them | fixture test, no panic |
| offline | tests read only `crates/ai-monitor/tests/fixtures/` | no `~/.claude` access |

## Gate 3 — Qualitative
- **Policy/mechanism split (SD4)** — no percentage, no formatted string, no per-model table, no display threshold anywhere in the crate.
- **Purity (D3)** — `apply` reads no clock and no filesystem; the snapshot exposes `first_event_ms`/`last_event_ms`, never `uptime_ms`/`idle_ms`.
- **Contract clarity (D11)** — the snapshot's field list matches the spec, including all six diagnostics, with the three orphan counters kept distinct rather than merged.
- **Spec alignment** — R0–R10 GWTs hold.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the declared surface above — the fold, the types, the fixtures, the inline tests.
**🛑 STOP — escalate (park):**
- Adding any dependency to `crates/ai-monitor/Cargo.toml` (SD6 — `std` + `serde_json` only).
- Any `wgpu` / `winit` / `renderer` / `app` import reaching this crate (SD2).
- Creating a `crates/ai-monitor/src/telemetry/` **directory** module — the declared surface is the bare path `telemetry.rs`.
- Editing any file outside the declared surface.
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Appended by the unattended run of 2026-07-28. `source: auto-unattended` means no
     human approved these at record time; review and prune. -->

### L1 — a bound test must fail when the bound is removed
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: For every cap/limit, the test asserting it must be mutation-checked — delete the cap and confirm that exact test fails. A test asserting a total an *unbounded* structure also produces proves nothing about the bound.
- note: `dedupe_cap_does_not_corrupt_totals` asserted `total_output == 2000`, which an uncapped set yields too; it passed with `DEDUPE_CAP` deleted. Replaced by a 65-then-replay test that observes eviction directly.

### L2 — every arithmetic path on external numbers must saturate
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: No `+`/`+=` on any value read from a log line. Token totals and derived sums use `saturating_add`, and a test folds `u64::MAX` in every numeric field.
- note: unchecked `+=` on the token totals panicked on `output_tokens: u64::MAX`, violating the contract's "never panic on any input". The overlay lives in the terminal's own process — an abort takes the terminal with it.

### L3 — a fix to the dedupe keying needs the full presence/order sweep
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: After ANY change to `fold_usage`'s keying, sweep every `requestId`-presence mask over a multi-line single request and both two-request interleavings, asserting totals AND `usage_conflicts`.
- note: three successive revisions each fixed the previous defect and introduced the next — a concatenated key split on mixed presence; keying on id alone left an uncapped per-key `Vec` and false-flagged split retries; requiring differing values closed the seam but broke approved requirement R1.2. The shipped rule discriminates on identity alone ("an unseen requestId on a key that already had one").

### L4 — script-written source bypasses the rustfmt hook
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: the PostToolUse rustfmt hook fires on Write/Edit, not on files written from a shell heredoc. Run `cargo fmt --all` before verifying when tests were generated by script, or Gate 1's format check fails on otherwise-correct work.

### L5 — residual: `usage_conflicts` can over-count after a retry
- cause: gate-fail
- source: auto-unattended
- form: context
- status: active
- check: —
- note: a replica arriving NON-contiguously after a retry compares against the rebased tuple and increments `usage_conflicts` by one. Totals are unaffected. Requires non-contiguous replicas, measured at zero occurrences corpus-wide. Worth a line in the spec's Known Gaps — a spec edit is outside this loop's declared surface, so it is recorded here for the human.
