# coherence-report.md — mac-terminal-v1

> Emitted by `coherence-audit` (T2). Flag-only: the checker WRITES this report and
> nothing else — it never edits or rewrites a `spec.md`. The human / calling skill
> (decompose) resolves every finding; the checker only surfaces them.

- generated_at: 2026-07-17 (re-audit after parser.rs resolution)
- verdict: OK
- specs_audited: 7   (declared: 7 | undeclared: 0)
- input_ref: [ai-monitor, glyph-rendering, image-paste, performance-harness, scrollback, settings, tabs] @ specs/{ai-monitor,glyph-rendering,image-paste,performance-harness,scrollback,settings,tabs}/spec.md

## Summary

| tier | classification | findings | severity |
|------|----------------|----------|----------|
| contradiction | incompatible (same role, conflicting choice) | 0 | **BLOCK** |
| overlap | unordered (no `depends_on` path between them) | 0 | **BLOCK** |
| redundancy | near-duplicate Requirements/goal | 0 | warn |
| undeclared | no `## Declared Surface` / empty `declared-surface:` | 0 | warn |

- pairs sharing a surface (non-empty glob intersection): 7 (all ordered)
- **block-tier findings: 0**  → verdict is `OK` (R4.1: BLOCK iff > 0)

## Findings

(none)

## Considered, not flagged   (informative — do NOT affect the verdict)

- **ordered overlaps** (a `depends_on` path makes the shared write sequential — NOT flagged):
  - tabs ⨯ glyph-rendering  (path: tabs → glyph-rendering)  shares `(crates/renderer/**, crates/renderer/src/tab_bar.rs)`
  - image-paste ⨯ glyph-rendering  (path: image-paste → glyph-rendering)  shares `(crates/renderer/**, crates/renderer/src/image_layer.rs)`
  - ai-monitor ⨯ glyph-rendering  (path: ai-monitor → glyph-rendering)  shares `(crates/renderer/**, crates/renderer/src/overlay_layer.rs)`
  - image-paste ⨯ tabs  (path: image-paste → tabs)  shares `(crates/app/**, crates/app/src/clipboard.rs)`
  - ai-monitor ⨯ tabs  (path: ai-monitor → tabs)  shares `(crates/app/**, crates/app/src/overlay.rs)`
  - settings ⨯ tabs  (path: settings → tabs)  shares `(crates/app/**, crates/app/src/settings.rs)`
  - image-paste ⨯ scrollback  (path: image-paste → scrollback)  shares `(crates/term-core/src/parser.rs, crates/term-core/src/parser.rs)` and `(crates/term-core/src/snapshot.rs, crates/term-core/src/snapshot.rs)`  ← the resolved coordination: scrollback owns term-core screen-state (grid+snapshot+scrollback+parser), image-paste extends parser.rs/snapshot.rs after it

- **complementary shares** (different roles on one path — NOT a conflict; independent judge subagent, maker ≠ checker):
  - glyph-rendering (owns the `DrawLayer`/`add_layer` seam) ⨯ tabs (authors `tab_bar.rs` as a layer through it) — glyph `D4` vs tabs `D3`
  - glyph-rendering (seam owner) ⨯ image-paste (authors `image_layer.rs` layer) — glyph `D4` vs image-paste `D3`
  - glyph-rendering (seam owner) ⨯ ai-monitor (authors `overlay_layer.rs` layer) — glyph `D4` vs ai-monitor `D4`
  - tabs (owns the `InputHandler`/`Overlay` SD4 seam) ⨯ image-paste (registers a Cmd+V handler) — tabs `D4` vs image-paste `D4`
  - tabs (SD4 seam owner) ⨯ ai-monitor (registers an `Overlay`/toggle) — tabs `D4` vs ai-monitor `D4`
  - tabs (SD4 seam owner) ⨯ settings (registers a loop participant) — tabs `D4` vs settings `D3`
  - scrollback (owns term-core screen-state; dispatches alt-screen `?1049h/l` in `parser.rs`, composes `view_offset` fields in `snapshot.rs`) ⨯ image-paste (adds Kitty-APC image dispatch to `parser.rs`, an image-layer field to `snapshot.rs`) — **different dispatch arms / different side-channel fields on the same files** (scrollback `D5`/`D3` vs image-paste `D1`/`D2`), additive and disjoint, ordered via `depends_on: scrollback`

## Verdict

**OK** — no findings (no contradictions, no unordered overlaps, no redundancy, no undeclared surfaces). All 7 pairs that share a surface are dependency-ordered, and an independent judge subagent classified every one as complementary (seam owner vs consumer; term-core screen-state owner vs ordered extender — different escape-dispatch arms and different snapshot side-channel fields), not a conflicting-choice contradiction. The prior parser.rs cross-feature coordination is now resolved inside the partition: `scrollback` owns `crates/term-core/src/parser.rs` + `snapshot.rs`, and `image-paste` extends them as an ordered share (`depends_on: scrollback`). The set is coherent for what is declared.
