# coherence-report.md — post-pr2-backlog

> Emitted by `coherence-audit` (T2). This is a **flag-only** artifact: the checker
> WRITES this report and nothing else — it never edits or rewrites a `spec.md`
> ("never push through the fence", R7.3). The human (or the calling skill —
> decompose) resolves every finding; the checker only surfaces them.

- generated_at: 2026-07-18
- verdict: OK
- specs_audited: 3   (declared: 3 | undeclared: 0)
- input_ref: [housekeeping, idle-cpu, vt-color-attrs] · specs/housekeeping/spec.md specs/idle-cpu/spec.md specs/vt-color-attrs/spec.md

## Summary

| tier | classification | findings | severity |
|------|----------------|----------|----------|
| contradiction | incompatible (same role, conflicting choice) | 0 | **BLOCK** |
| overlap | unordered (no `depends_on` path between them) | 0 | **BLOCK** |
| redundancy | near-duplicate Requirements/goal | 0 | warn |
| undeclared | no `## Declared Surface` / empty `declared-surface:` | 0 | warn |

- pairs sharing a surface (non-empty glob intersection): 1  (idle-cpu ⨯ vt-color-attrs — ordered)
- **block-tier findings: 0**  → verdict is `BLOCK` iff this is > 0 (R4.1)

## Findings   (R3.3 — one block per conflicting pair / flagged spec)

(none — no block-tier or warn-tier findings)

## Considered, not flagged   (informative — audit completeness; these do NOT affect the verdict)

- **ordered overlaps** (a `depends_on` path makes the shared write sequential — D2/R2.3, NOT flagged):
  - idle-cpu ⨯ vt-color-attrs  (path: vt-color-attrs → idle-cpu)  shares `(crates/renderer/src/lib.rs, crates/renderer/src/lib.rs)`
    — idle-cpu owns `render()`/`draw_to` for its snapshot-equality cache; vt-color-attrs
    extends the same file's `build_bg_instances` / `build_glyph_instances` to read cell
    attributes, *after* idle-cpu (ordered by the `depends_on` edge).
- **complementary shares** (different roles on one path — D7/R3.2, NOT a conflict):
  - idle-cpu (caches the instance-builder output, keyed on `GridSnapshot ==` — `D5`/`D6`)
    ⨯ vt-color-attrs (changes what the instance builders emit by reading `cell.attrs` — `D9`)
    under `crates/renderer/src/lib.rs`.
    Independent judge (maker ≠ checker) verdict: **complementary → ok**. Both specs carry the
    IDENTICAL inherited `SD2` (GridSnapshot is the versioned contract AND the frame-equality
    basis) as common ground; vt-color-attrs stores attributes inside `Cell` with a derived
    `PartialEq` (its `D5`), so the equality cache invalidates on attribute-only changes — the
    two decisions compose (D5 decides *whether* the builders run; D9 decides *what* they emit).

### Pairs with no shared surface (no contradiction possible)
- idle-cpu ⨯ housekeeping — glob intersection ∅ (`crates/**` vs `docs/**` + `CLAUDE.md`).
- vt-color-attrs ⨯ housekeeping — glob intersection ∅ (`crates/**` vs `docs/**` + `CLAUDE.md`).

## Verdict

**OK** — no findings (no block-tier contradiction or unordered overlap; no warn-tier
redundancy or undeclared surface). The set is coherent for what is declared: the single
shared surface (`crates/renderer/src/lib.rs`) is dependency-ordered and judged
complementary by an independent contradiction judge.
