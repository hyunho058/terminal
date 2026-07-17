# loop.md — Loop Contract: performance-harness

- pre-approved-unattended: yes

## Goal
Add Criterion benchmarks for the hot paths, record confirmed numeric targets, and gate them in a required CI workflow, per `specs/performance-harness/spec.md`, without violating it or the Shared Decisions.

## Scope & non-goals
- In: `benches/**`, `.github/workflows/perf.yml`, `docs/performance.md`
- Out: all crate source (this feature measures/gates; it never edits another feature's code to optimize it).

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| benches build | `cargo bench --no-run` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| bench groups present | 3 (input→grid, frame, 100k-scroll) | `cargo bench -- --list` |
| perf thresholds | meet committed `docs/performance.md` numbers (provisional until G1 confirmed) | `perf.yml` comparison step |

## Gate 3 — Qualitative
- **Architecture fit** — benches run headless; `perf.yml` is separate from `ci.yml`, reads absolute thresholds from `docs/performance.md`.
- **Spec alignment** — R0–R3 GWTs hold (three bench groups, recorded targets, required-check gate, comparison-step bridge).
- **Naming / readability** — the comparison step + thresholds doc are clear.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within `benches/**`, `perf.yml`, `docs/performance.md`.
**🛑 STOP — escalate (park):**
- **Confirming the G1 numeric targets** (120fps / 8ms / 100k-scroll are *examples* — the committed thresholds are a human decision). Draft the doc with placeholders + measured baselines, then escalate for the human to confirm the numbers.
- Editing any crate source to make a benchmark pass.
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.
