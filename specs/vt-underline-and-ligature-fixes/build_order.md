# build_order.md — vt-underline-and-ligature-fixes

> Cross-feature ledger for the two-feature bug-fix batch produced by /decompose.
> Durable resume anchor for the run. Written atomically (temp + rename).

- generated_at: 2026-07-19
- approved_at: 2026-07-19
- approved_by: user (batch approval — plan + both gate contracts + commit strategy)
- coherence_verdict: OK   # specs/vt-underline-and-ligature-fixes/coherence-report.md
- input_ref: sgr-private-intermediate-guard::specs/sgr-private-intermediate-guard/spec.md::goal=stop-persistent-underline-from-XTMODKEYS-misparse + ligatures-default-off::specs/ligatures-default-off/spec.md::goal=stop-fi-ghost-space-via-ligatures-default-off
- build_branch: fix/vt-underline-and-ligature   # human chose to commit on the CURRENT (non-main) branch; no separate build/ branch
- base_ref: ec4a1ca024d11c3e2ead049d5f16a771d465be2c   # HEAD at arming
- commit_on_green: true   # commit each green feature on build_branch; NO push, NO merge, never main
- push: false

## Features  (topological order — a dependency must appear before its dependents)

### sgr-private-intermediate-guard
- spec: specs/sgr-private-intermediate-guard/spec.md
- depends_on: []
- status: done
- reason: —
- accept: Feeding `ESC[>4;2m` (XTMODKEYS) through the processor leaves the following cells' UNDERLINE attribute OFF — the CSI `m` handler ignores the private intermediates `>`/`<`/`=` instead of treating them as SGR; a regression test in parser.rs proves it.
- commit: 288885a62962a7aae0027b5bc131e1c5c77f1150
- pushed: false

### ligatures-default-off
- spec: specs/ligatures-default-off/spec.md
- depends_on: []
- status: done
- reason: —
- accept: With the default config, `font.ligatures` is `false`, so typing `first` renders as five discrete monospace cells with no ghost space; a `common` unit test asserts `FontConfig::default().ligatures == false`.
- commit: 85061809d4353442372abc1095ccb7e9796b26aa
- pushed: false

<!--
Notes:
- Both features are roots (depends_on: []) — the ready set is {both} from the start;
  they own DISJOINT surfaces (parser.rs vs config.rs) so no ordering is required.
- `blocked` is never written here — computed each cycle. `parked` is sticky (human un-parks).
- commit_on_green: true, on branch fix/vt-underline-and-ligature (non-main); staging is
  surface-scoped (only the feature's declared globs), so untracked specs/ artifacts and the
  sibling feature's files are excluded automatically. No push. The merge is left to the human.
-->
