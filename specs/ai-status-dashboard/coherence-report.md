# coherence-report.md — ai-status-dashboard

- verdict: OK
- generated_at: 2026-07-28
- specs_audited: 4
- input_ref: [overlay-dashboard-boxes, overlay-dashboard-frame, telemetry-model, telemetry-watcher] · specs/overlay-dashboard-boxes/spec.md specs/overlay-dashboard-frame/spec.md specs/telemetry-model/spec.md specs/telemetry-watcher/spec.md
- contradiction_tier: judged by a separate `Agent` subagent (maker ≠ checker); it read the four specs' `## Decisions` + `## Constraints` and the real source under `crates/`, and was explicitly barred from the partition manifest

## Summary

| Tier | Findings | Severity |
|------|----------|----------|
| Contradiction (incompatible Decisions on a shared surface) | 0 | block |
| Unordered overlap (independent features, same surface) | 0 | block |
| Redundancy (near-duplicate Requirements/goal) | 0 | warn |
| Undeclared surface | 0 | warn |

All four specs carry a `## Declared Surface` with a non-empty `declared-surface:` list and a
`depends_on` field, so the overlap check was fully verifiable — no spec degraded to the
`surface undeclared; overlap unverifiable — declare to enable the check` warning.

## Findings

(none)

## Considered, not flagged

### Ordered overlaps — legitimate shared surfaces

The feature graph is a linear chain, so every pair is ordered (directly or transitively) and no
overlap is flaggable by construction:

```
telemetry-model → telemetry-watcher → overlay-dashboard-frame → overlay-dashboard-boxes
```

| Pair | Shared glob | Ordering | Why it is legitimate |
|------|-------------|----------|----------------------|
| telemetry-model ⨯ telemetry-watcher | `crates/ai-monitor/src/lib.rs` | direct edge | model declares the `telemetry` module and its types; watcher then rewrites `LogWatcher`'s payload in the same file, after |
| telemetry-watcher ⨯ overlay-dashboard-frame | `crates/app/src/overlay.rs` | direct edge | watcher replaces the event feed with a snapshot field + placeholder render; frame then replaces only that placeholder call site |
| overlay-dashboard-frame ⨯ overlay-dashboard-boxes | `crates/app/src/dashboard.rs` | direct edge | frame creates the file, the composer entry point and the layout constants; boxes appends sections consuming them |

Disjoint pairs (empty glob intersection, nothing to gate): telemetry-model ⨯ overlay-dashboard-frame,
telemetry-model ⨯ overlay-dashboard-boxes, telemetry-watcher ⨯ overlay-dashboard-boxes.

> **Caveat on the strength of this tier.** Because the graph is a straight chain, the overlap check
> *cannot* fail here — there is no unordered pair for it to flag. This tier therefore provides no
> evidence about this particular set; it is the contradiction tier below that did the work.

### Complementary shares — judged, not conflicts

The independent judge classified all three shared-surface pairs **complementary**:

- **telemetry-model ⨯ telemetry-watcher** on `crates/ai-monitor/src/lib.rs` — `D11` *defines* the
  snapshot type and its field list; `W1`/`W2a` *transport* it. `Arc<T>` derefs to `&T` and
  `Arc<T>: Send` holds under `T: Send + Sync`, so `W2a`'s trait pins are strictly additive to
  `D11`, not a competing declaration. `W1` explicitly distinguishes itself from the
  `Arc<Mutex<SessionTelemetry>>` that `SD5` rejects (immutable frozen view, no lock, watcher
  remains sole owner). `W2` keeps `backfill` alive, satisfying telemetry-model's Constraints pin —
  it disputes the pin's stated *reason*, not its conclusion. → **ok**
- **telemetry-watcher ⨯ overlay-dashboard-frame** on `crates/app/src/overlay.rs` — `W5` creates
  `latest: Option<Arc<TelemetrySnapshot>>` and declares its own rendering a placeholder for this
  sibling to replace; `F5` replaces exactly that call site and leaves the field, its owner and the
  `Overlay` trait signatures alone. `F5`'s `&TelemetrySnapshot` is reached from the `Arc` by
  `Deref`. Frame's `Cargo.toml`/`insta` addition is precisely what watcher's Constraints defer to
  it. → **ok**
- **overlay-dashboard-frame ⨯ overlay-dashboard-boxes** on `crates/app/src/dashboard.rs` — frame
  defines `RULE_COLS` and the composer entry point (`F1`, `F5`); boxes consumes both and adds
  sections (`B2a`, `B7`) and decides section order under SD4 (`B1`). Boxes defines no constant
  frame defines and claims no entry point. → **ok**

### Duplicated verification requirements — deliberate, not redundancy

All four specs carry a near-identical "workspace gates stay green" sub-requirement
(`cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`):
telemetry-model R10.3, telemetry-watcher R7.3, overlay-dashboard-frame R7.4,
overlay-dashboard-boxes R8.3.

This is textually near-duplicate but is **not flagged as redundancy**. The redundancy tier exists
to catch duplicated *scope* — two specs building the same thing. Here each feature is driven
through its own `/loop` gate independently, so each must assert the gate for itself; removing it
from three of the four would leave those features with no Gate-1 requirement to verify against.
Recorded here rather than suppressed, since the call is a judgement.

## Intra-spec defects surfaced by the judge (out of tier, resolved by the maker)

Neither is a cross-spec finding and neither affects the verdict, but both were real and are
recorded for audit completeness. The checker did not edit any spec — these were fixed by the
maker after the judgement, and the verdict above reflects the corrected files:

1. **overlay-dashboard-frame F3a** carried a stale `RULE_COLS = 96` in its worked-example preamble
   while `F1` — the defining decision — fixes it at `104`. Boxes had consistently consumed `104`.
   Corrected to `104`.
2. **overlay-dashboard-boxes B7** stated an 85-column path budget as "78" with a chrome enumeration
   summing to 19, not the "≈26" it claimed. Corrected to `85` with the arithmetic written out term
   by term. Notably this was the *second* time that decision's number disagreed with its own
   derivation — the same defect it accuses its earlier revision of.

## Verdict

**OK** — no findings.

Rule applied: `BLOCK` iff ≥1 block-tier finding (a contradiction OR an unordered overlap); `WARN`
iff no block-tier but ≥1 warn-tier finding; otherwise `OK`. Zero contradictions (three shared pairs,
all judged complementary by an independent subagent), zero unordered overlaps (the graph is a chain,
so none is possible), zero undeclared surfaces, zero redundancy findings.

The set is coherent **for what is declared**. Two limits on that statement, stated plainly:
the overlap tier was vacuous on a linear chain, and coherence is judged from the specs' declared
surfaces and decisions — it says nothing about whether the plan is *correct*, only that the four
specs do not contradict each other or collide on a file.
