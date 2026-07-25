# coherence-report.md — clipboard-copy-paste

> Emitted by `coherence-audit` (decompose-end, maker ≠ checker on decompose's own
> output). Flag-only: this report is the only artifact written; no `spec.md` was
> edited or rewritten. The overlap + redundancy tiers are deterministic (maker-run);
> the contradiction tier was judged by a SEPARATE independent subagent that read only
> the spec files — never the decomposer's reasoning or the partition manifest.

- generated_at: 2026-07-24T00:11:18-0700
- verdict: OK
- specs_audited: 5   (declared: 5 | undeclared: 0)
- input_ref: [copy-clipboard, selection-highlight, selection-input, selection-model, text-paste] @ specs/{copy-clipboard,selection-highlight,selection-input,selection-model,text-paste}/spec.md

## Summary

| tier | classification | findings | severity |
|------|----------------|----------|----------|
| contradiction | incompatible (same role, conflicting choice) | 0 | **BLOCK** |
| overlap | unordered (no `depends_on` path between them) | 0 | **BLOCK** |
| redundancy | near-duplicate Requirements/goal | 0 | warn |
| undeclared | no `## Declared Surface` / empty `declared-surface:` | 0 | warn |

- pairs sharing a surface (non-empty glob intersection): 0
- **block-tier findings: 0**  → verdict is `OK` (no findings)

## Findings   (R3.3 — one block per conflicting pair / flagged spec)

(none)

## Considered, not flagged   (informative — audit completeness; these do NOT affect the verdict)

- **ordered overlaps** (a `depends_on` path makes a shared write sequential — NOT flagged): none — the 5 surfaces are **fully disjoint** (13 distinct files, every file single-owner), so no pair shares any file, ordered or unordered.
- **complementary shares** (different roles on one path — NOT a conflict): none (no shared paths).
- **interface-contract coherence** (checked by the independent judge because the file-disjoint set is still coupled along `depends_on` edges — a producer/consumer mismatch would break the build without any shared file): all 5 cross-feature contracts judged **COMPATIBLE**, no dangling contracts —
  - Selection type + `GridSnapshot.selection` / `Selection::contains` / `selection_text` — selection-model D6/D7/D9 (producer) ↔ selection-highlight D7, selection-input D9, copy-clipboard D6 (consumers): viewport coords + membership agree (highlight tints continuation cells, extraction skips them — a content property, not a membership divergence).
  - `Terminal::set_selection`/`clear_selection`/`bracketed_paste`/`snapshot` — selection-model R2/R4 (producer) ↔ selection-input D8/D9 (consumer): agree; `snapshot()` pre-exists (`term-core/src/lib.rs:59`), not a dangling contract.
  - `AppCtx.bracketed_paste` + `AppCtx.selection_text` — selection-input D9 (producer) ↔ text-paste D6/D7, copy-clipboard D6 (consumers): names/types/defaults match; copy-clipboard pins population at `AppCtx` construction (not a router side effect) → no registration-order contradiction.
  - `renderer::selection_layer::SelectionLayer` + `SELECTION_Z` — selection-highlight D6/D8 (producer) ↔ copy-clipboard D8 (consumer): identical module path + `new()`; consumer references the `SELECTION_Z` symbol, not a literal.
  - Bracketed-paste 2004 → `ESC[200~`/`ESC[201~` — selection-model SD5/R4 ↔ text-paste D7/R2: consistent enable-flag → delimiter flow.

## Verdict

**OK** — no findings, R4-none. The set is coherent for what is declared: the declared surfaces are fully disjoint (overlap tier empty), the independent contradiction judge returned CONTRADICTIONS: 0 / INTERFACE-COHERENT across all producer↔consumer edges, and there is no near-duplicate requirement or undeclared surface.

> Note (out of coherence-audit's cross-spec scope, surfaced for the maker): the judge
> observed one *intra*-spec inconsistency in selection-highlight — task T2 named
> `SELECTION_Z = 5` while D8/R1.1 use `4`. That is a single-spec defect, not a
> cross-spec conflict, so it does not affect this verdict; it was corrected in the spec
> after the audit (T2 → `SELECTION_Z = 4`, matching the reviewed decision).
