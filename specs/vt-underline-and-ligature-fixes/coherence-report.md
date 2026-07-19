# coherence-report.md — vt-underline-and-ligature-fixes

> Emitted by `coherence-audit` (T2). This is a **flag-only** artifact: the checker
> WRITES this report and nothing else — it never edits or rewrites a `spec.md`
> ("never push through the fence", R7.3). The human (or the calling skill —
> build-order / decompose) resolves every finding; the checker only surfaces them.
> The per-pair block below is the R3.3 verdict schema: `{verdict, cited evidence, block|warn}`.

- generated_at: 2026-07-19
- verdict: OK
- specs_audited: 2   (declared: 2 | undeclared: 0)
- input_ref: ligatures-default-off::specs/ligatures-default-off/spec.md + sgr-private-intermediate-guard::specs/sgr-private-intermediate-guard/spec.md

## Summary

| tier | classification | findings | severity |
|------|----------------|----------|----------|
| contradiction | incompatible (same role, conflicting choice) | 0 | **BLOCK** |
| overlap | unordered (no `depends_on` path between them) | 0 | **BLOCK** |
| redundancy | near-duplicate Requirements/goal | 0 | warn |
| undeclared | no `## Declared Surface` / empty `declared-surface:` | 0 | warn |

- pairs sharing a surface (non-empty glob intersection): 0
- **block-tier findings: 0**  → verdict is `BLOCK` iff this is > 0 (R4.1)

## Findings   (R3.3 — one block per conflicting pair / flagged spec)

(none)

## Considered, not flagged   (informative — audit completeness; these do NOT affect the verdict)

- **overlap tier** — the single pair `sgr-private-intermediate-guard ⨯ ligatures-default-off`
  was checked by glob set-intersection: `crates/term-core/src/parser.rs` vs
  `crates/common/src/config.rs` → **empty intersection** (two distinct bare-path literals,
  no wildcards). No shared surface, so nothing to flag or gate.
- **contradiction tier** — runs only over pairs sharing a surface; there are **0** such
  pairs, so there are **no contradiction candidates**. The inherited `## Shared Decisions`
  (`SD1`/`SD2`/`SD3`) are **byte-identical** in both specs → shared common ground, never a
  conflict (D7). Each spec's feature-local decisions act on its own disjoint file
  (parser.rs vs config.rs), so no divergent decision meets on a shared path. No `Agent`
  judge was spawned because no shared surface exists to hand it (this is not the
  no-`Agent`-tool fallback — the tier has genuinely zero candidates).
- **ordered overlaps**: (none — the only pair is unordered but non-overlapping)
- **complementary shares**: (none)

## Verdict

**OK** — no findings, OK (R4.2 not triggered — zero warn-tier findings; R4.1 not
triggered — zero block-tier findings). The two features own strictly disjoint surfaces
(`crates/term-core/src/parser.rs` and `crates/common/src/config.rs`), carry identical
inherited Shared Decisions, and have non-overlapping goals/requirements. The set is
coherent for what is declared.

> If **BLOCK** and the caller is an unattended / autopilot path: this **HARD-STOPS** the run.
> A WARN there would be an unread annotation (approval was pre-batched), so a known write-collision
> or contradiction must not proceed unseen (R4.3). A human resolves it; the checker never auto-resolves.
