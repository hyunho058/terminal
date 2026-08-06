# loop.md — Loop Contract

> Supervisor document for **atlas-swap-bind-invalidation**. Verifies an already-complete
> implementation against `spec.md` in this directory. Approved by the user before the run.

## Goal

Every layer that samples the glyph atlas must rebuild its bind group when the atlas
*texture* is replaced — and only then. Must not violate
`specs/atlas-swap-bind-invalidation/spec.md` (D1–D8, R0–R6, Constraints, Known Gaps).

## Scope & non-goals

- **In**: `crates/renderer/src/{atlas,sidebar_row,overlay_layer,preedit_layer}.rs`,
  comments in `crates/renderer/src/lib.rs`, and tests for the above.
- **Out**: the grid render path's behaviour (D7/G-a), `apply_config` call frequency and
  the config-watcher echo suppression, the unconditional atlas rebuild itself (G-b),
  intra-frame eviction (G-c), the ppem/cell mismatch on allocation failure (G-d).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| fmt | `cargo fmt --all -- --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

> No separate typecheck step — `cargo build` typechecks. N/A by language design.

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| new regression tests passing | `9/9` | `cargo test -p renderer --lib` filtered on the new test names |
| layers with an epoch-keyed bind cache | `3/3` | `grep -c 'atlas.epoch()'` across the three layer files |
| atlas texture creation sites | `== 1` | `grep -c 'create_texture' crates/renderer/src/atlas.rs` |
| spec sub-requirements traced | `26/26` | manual trace against `spec.md` `## Requirements` |

> The texture-creation-site metric is D1's by-construction invariant made enforceable: a
> future `Atlas::resize()` that swaps the texture outside the factory fails this gate
> instead of silently freezing `epoch` and reintroducing the bug (G-b's warning).

## Gate 3 — Qualitative  (score + grounds + action)

Scored by a **separate checker subagent** (maker ≠ checker) given only the diff, this
contract, and `spec.md`.

- **Test integrity** — do the swap tests genuinely fail when the fix is reverted, and is
  every vacuous-pass trap cleared? (spec R5.2–R5.4 exist solely to prevent tests that
  pass without exercising anything: three distinct early returns, the `thread_local`
  sidebar singleton, and epoch-value assertions that would be flaky.)
- **Spec alignment** — R0–R6 honoured as written, including the deliberate *removals*
  (`generation` and `cell` dropped from the key, not replaced).
- **Architecture fit** — one shared invalidation idiom across three layers; matches crate
  conventions (comment density, `pub(crate)` vs `pub`, test-skip convention).
- **Constraint compliance** — bind-group rebuilds occur only on texture swap, never
  per-frame (the G1 idle-CPU debt); scope discipline held (per-frame instance buffers
  untouched, grid path unchanged, no `unwrap()` outside tests).

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests,
docs/comments, local renames, behaviour-preserving refactors.

**🛑 STOP — call a human (escalate):**
- DB schema changes · data-loss-capable migrations · auth/permission changes ·
  payment/security-sensitive changes (none plausible in this scope, listed for completeness)
- **Anything conflicting with `spec.md`** — specifically: re-adding `generation` or `cell`
  to the bind key (D2 removed them deliberately), altering the grid bind-group path (D7),
  changing when `apply_config` fires (Constraints), or making `text_bind` an `Option`
  (D3 rule 4, which would force an `unwrap()` at the draw site).

## Exit criteria

Exits only when **every** gate passes and the Evidence Report is emitted. A 🛑 boundary
ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Phase 0 loads `status: active` lessons. The human owns this section.

<!-- A fresh contract has none. -->

### L1 — key-equality assertions have no power against per-frame rebuilds
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "Test integrity — for every 'X must not happen' assertion, verify by mutation that breaking X actually fails the test. Key/state equality alone cannot express 'this did not happen N times'; a counter or equivalent is required."
- note: iteration 1's three idle tests asserted key equality and still passed with the bind-group short-circuit deleted — they had zero power against the exact G1 regression they were named for.

### L2 — a proxy adjacent to an assignment can always be decoupled by a mutant
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: rebuild counters sit *next to* `bind = ...`, so deleting only the assignment leaves the count intact and the swap tests green; closing this needs pixel readback, not a better proxy (Known Gap G-f).
