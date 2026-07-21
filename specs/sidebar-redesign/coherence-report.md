# coherence-report.md — sidebar-redesign

> Emitted by `coherence-audit` (T2). This is a **flag-only** artifact: the checker
> WRITES this report and nothing else — it never edits or rewrites a `spec.md`
> ("never push through the fence", R7.3). The human (or the calling skill —
> build-order / decompose) resolves every finding; the checker only surfaces them.
> The per-pair block below is the R3.3 verdict schema: `{verdict, cited evidence, block|warn}`.

- generated_at: 2026-07-20
- verdict: OK
- specs_audited: 5   (declared: 5 | undeclared: 0)
- input_ref: [sidebar-chrome-collapse, sidebar-foundations, sidebar-interaction, sidebar-row-redesign, tab-metadata] @ specs/{sidebar-foundations,tab-metadata,sidebar-row-redesign,sidebar-chrome-collapse,sidebar-interaction}/spec.md

## Summary

| tier | classification | findings | severity |
|------|----------------|----------|----------|
| contradiction | incompatible (same role, conflicting choice) | 0 | **BLOCK** |
| overlap | unordered (no `depends_on` path between them) | 0 | **BLOCK** |
| redundancy | near-duplicate Requirements/goal | 0 | warn |
| undeclared | no `## Declared Surface` / empty `declared-surface:` | 0 | warn |

- pairs sharing a surface (non-empty glob intersection): 6
- **block-tier findings: 0**  → verdict is `BLOCK` iff this is > 0 (R4.1); it is 0 → not BLOCK

## Findings   (R3.3 — one block per conflicting pair / flagged spec)

(none — no block-tier or warn-tier findings)

## Considered, not flagged   (informative — audit completeness; these do NOT affect the verdict)

- **ordered overlaps** (a `depends_on` path makes the shared write sequential — D2/R2.3, NOT flagged):
  - sidebar-foundations ⨯ sidebar-row-redesign  (path: sidebar-row-redesign → sidebar-foundations)  shares `(crates/renderer/src/lib.rs, crates/renderer/src/lib.rs)`
  - sidebar-foundations ⨯ sidebar-chrome-collapse  (path: sidebar-chrome-collapse → sidebar-foundations)  shares `(crates/renderer/src/lib.rs, crates/renderer/src/lib.rs)`
  - sidebar-row-redesign ⨯ sidebar-chrome-collapse  (path: sidebar-chrome-collapse → sidebar-row-redesign)  shares `(crates/renderer/src/lib.rs, crates/renderer/src/lib.rs)`
  - tab-metadata ⨯ sidebar-row-redesign  (path: sidebar-row-redesign → tab-metadata)  shares `(crates/app/src/main.rs, crates/app/src/main.rs)`
  - tab-metadata ⨯ sidebar-interaction  (path: sidebar-interaction → sidebar-row-redesign → tab-metadata, transitive)  shares `(crates/app/src/main.rs, crates/app/src/main.rs)`
  - sidebar-row-redesign ⨯ sidebar-interaction  (path: sidebar-interaction → sidebar-row-redesign)  shares `(crates/app/src/main.rs, crates/app/src/main.rs)`

- **complementary shares** (different roles on one path — D7/R3.2, NOT a conflict; judged by the independent contradiction subagent):
  - sidebar-foundations (adds 3 `pub mod` + minimal inert plumbing per D10) ⨯ sidebar-row-redesign (adds `pub mod sidebar_row`, `sync_left_inset`, setters; appends after foundations' `apply_config` line per D4) under `crates/renderer/src/lib.rs` — distinct additive symbols; dependent builds on foundations' `FrameCtx` fields rather than redefining them
  - sidebar-foundations (D10) ⨯ sidebar-chrome-collapse (adds only `pub mod sidebar_layout;` + `pub mod sidebar_header;`, registers nothing from lib.rs per D14) under `crates/renderer/src/lib.rs` — disjoint mod lines; D14 enumerates and preserves foundations' budget
  - sidebar-row-redesign (D4 owns `set_left_inset`/`sync_left_inset`/setters) ⨯ sidebar-chrome-collapse (D14 two mod lines, reads `ctx.left_inset` via D3 without editing it) under `crates/renderer/src/lib.rs` — no shared symbol redefined; the z-order divergence (row-redesign `TAB_BAR_Z=10` vs chrome-collapse `SIDEBAR_CHROME_Z=30`) is two different layers' own constants, named and reconciled, and lives in `terminal_app.rs`, off this shared file
  - tab-metadata (D3 registers `TabMetaPump` from main.rs, takes `proxy.clone()` at `main.rs:44`; D12 assigns the `set_enabled`+config-read edit to sidebar-row-redesign by name) ⨯ sidebar-row-redesign (D10 installs `SidebarMetaSource` via `TabModelHandle::shared().set_meta_source`) under `crates/app/src/main.rs` — two different objects on different lines, ownership explicitly partitioned
  - tab-metadata (D3 owns the `main.rs:44` → `proxy.clone()` edit + pump registration) ⨯ sidebar-interaction (D12: empty main.rs diff — "verify, not repeat"; defers to tab-metadata's ownership) under `crates/app/src/main.rs` — interaction declines to touch what tab-metadata owns
  - sidebar-row-redesign (D10 sole writer of main.rs content in this pair) ⨯ sidebar-interaction (D12 adds nothing to main.rs; its layer registration `D4` targets `terminal_app.rs`) under `crates/app/src/main.rs` — disjoint

## Verdict

**OK** — no findings. The set is coherent for what is declared.

- Overlap tier (deterministic, maker-run): all 6 surface-sharing pairs are dependency-ordered (direct or transitive `depends_on` path); 0 unordered overlaps → 0 block-tier overlap findings.
- Contradiction tier (judged, separate `Agent` subagent — maker ≠ checker): all 6 shared-surface pairs classified COMPLEMENTARY with cited Decision evidence; `JUDGE VERDICT: OK`.
- Redundancy tier (advisory, maker-run): no near-duplicate feature Requirements/goal. The `R0` "three gates green / visually inert" verification scaffolding recurs per spec by design (each feature's own acceptance), which is intentional repetition, not duplicated feature work — not flagged.
- Undeclared tier: 0 — every spec carries a `## Declared Surface` with a non-empty `declared-surface:` list matching the partition manifest.

> This verdict certifies **cross-spec coherence for what is declared** — it does NOT re-run the
> within-spec L2 review. Note (surfaced by decompose, not by this checker): the batch run that
> generated `tab-metadata`, `sidebar-row-redesign`, `sidebar-chrome-collapse` and
> `sidebar-interaction` was interrupted by a session rate limit; each recorded a passing L2-reviewer
> verdict but their final round-3 non-blocking fixes were applied without a confirming review pass
> (see each spec's Meta `L2 review` line). That is a within-spec concern this cross-spec audit does
> not cover.
