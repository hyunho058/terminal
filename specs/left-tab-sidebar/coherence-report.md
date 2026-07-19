# coherence-report.md — left-tab-sidebar

> Emitted by `coherence-audit` (T2). This is a **flag-only** artifact: the checker
> WRITES this report and nothing else — it never edits or rewrites a `spec.md`
> ("never push through the fence", R7.3). The human (or the calling skill —
> build-order / decompose) resolves every finding; the checker only surfaces them.
> The per-pair block below is the R3.3 verdict schema: `{verdict, cited evidence, block|warn}`.

- generated_at: 2026-07-19T07:35:21Z
- verdict: OK
- specs_audited: 3   (declared: 3 | undeclared: 0)
- input_ref: renderer-left-inset (specs/renderer-left-inset/spec.md) + vertical-tab-sidebar (specs/vertical-tab-sidebar/spec.md) + tab-click-switch (specs/tab-click-switch/spec.md)
- invoked_by: build-order Plan (step 5) — second of the two idempotent audit stages (first was decompose-end)

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

(none — no contradiction, overlap, redundancy, or undeclared findings.)

## Considered, not flagged   (informative — audit completeness; these do NOT affect the verdict)

- **declared surfaces are fully disjoint** — every pairwise glob set-intersection is empty, so there is no shared surface at all:
  - renderer-left-inset `{crates/renderer/src/lib.rs, crates/renderer/src/cells.wgsl, crates/renderer/src/glyph.wgsl}`
  - vertical-tab-sidebar `{crates/renderer/src/tab_bar.rs}`
  - tab-click-switch `{crates/app/src/terminal_app.rs, crates/app/src/input.rs, crates/app/src/settings.rs}`
- **ordered overlaps** (a `depends_on` path makes a shared write sequential — D2/R2.3): none — no pair shares a surface. For the record, all three pairs ARE dependency-ordered:
  - renderer-left-inset ⨯ vertical-tab-sidebar  (path: vertical-tab-sidebar → renderer-left-inset)
  - renderer-left-inset ⨯ tab-click-switch  (path: tab-click-switch → renderer-left-inset)
  - vertical-tab-sidebar ⨯ tab-click-switch  (path: tab-click-switch → vertical-tab-sidebar)
- **complementary shares** (different roles on one path — D7/R3.2): none (no shared path).
- **cross-feature coupling is API-level and ordered, not surface-level**: features interact through forward-flowing symbols along the `depends_on` edges — `FrameCtx::left_inset` / `set_left_inset` and `tab_bar::{width_px, tab_height_px, slot_y, tab_at}` — never by editing the same file. Not a declared-surface overlap. API-signature consistency was checked within-spec by the L2-reviewers.

## Verdict

**OK** — no findings (no contradiction, no unordered overlap, no redundancy, no undeclared surface). The set is coherent for what is declared: three fully disjoint declared surfaces wired by a topologically-ordered `depends_on` DAG (renderer-left-inset → vertical-tab-sidebar → tab-click-switch), inheriting one identical `## Shared Decisions` block (SD1–SD4) as common ground.

> If **BLOCK** and the caller is an unattended / autopilot path: this **HARD-STOPS** the run.
> A WARN there would be an unread annotation (approval was pre-batched), so a known write-collision
> or contradiction must not proceed unseen (R4.3). A human resolves it; the checker never auto-resolves.
