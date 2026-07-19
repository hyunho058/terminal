# partition-manifest.md — vt-underline-and-ligature-fixes
- generated_at: 2026-07-19
- goal: Fix two independent bugs surfaced on branch fix/vt-underline-and-ligature — (Bug1) the whole screen shows a persistent underline because CSI `m` misparses the XTMODKEYS private sequence `ESC[>4;2m` as SGR 4 (underline), and (Bug2) a "ghost space" appears inside words (e.g. `first` → `fi rst`) because Menlo's typographic `fi` f-ligature is enabled by default and breaks the monospace cell grid.

## Shared Decisions
### SD1: Each fix stays inside its owning crate; the dependency direction (common ← term-core ← renderer ← app) and term-core's GPU-free invariant are preserved.
- **Status**: resolved
- **Rationale**: The two bugs live in different crates (`term-core` parser vs `common` config) and are fixed independently. Neither fix may reach across the crate boundary. Bug2's root symptom is visible in the renderer (`renderer/src/lib.rs:850` draws the shaped ligature glyph only at the lead cell), but the corrective change is a config default in `common` — the renderer is left byte-unchanged. Rejected: "fix" ligatures in the renderer by suppressing lead-cell-only ligature draws — that would entangle two crates, add rendering complexity, and still leave the misleading default on; flipping the default is the minimal, correct fix per the bug report.

### SD2: No new dependencies; reuse the existing vte / serde stack and mirror the code idiom already present at each change site.
- **Status**: resolved
- **Rationale**: Bug1's guard mirrors the existing private-mode guard at `parser.rs:261` (`intermediates.first() == Some(&b'?')`), extended to the SGR-relevant private intermediates `>`/`<`/`=`. Bug2 mirrors the existing `#[serde(default)] FontConfig::default()` pattern — a one-field default flip. Rejected: pulling a richer VT-parsing crate or a font-shaping abstraction — both are over-engineering for two localized fixes and would violate the no-GUI-crates / minimal-dependency posture.

### SD3: Every fix ships with a regression test in the owning crate's existing test convention, and the set must pass `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
- **Status**: resolved
- **Rationale**: Bug1's regression test goes in `parser.rs`'s existing `#[cfg(test)] mod tests` (where the SGR/CSI tests already live) — it drives `ESC[>4;2m` through the processor and asserts the cell attrs carry no UNDERLINE. Bug2's regression test asserts `FontConfig::default().ligatures == false` in a `common` unit test. Snapshot tests (`term-core/tests/grid_snapshot.rs`) are not required — these are boolean/attr assertions, not grid-shape changes. Rejected: manual-only verification — the underline bug is a silent regression that only a persisted test prevents from recurring.

## Features  (ordered — a dependency appears before its dependents)
### sgr-private-intermediate-guard
- declared-surface:
  - crates/term-core/src/parser.rs
- depends_on: []
- accept-seed: Feeding `ESC[>4;2m` (XTMODKEYS) through the processor leaves the following cells' UNDERLINE attribute OFF — the CSI `m` handler ignores the private intermediates `>`/`<`/`=` instead of treating them as SGR, mirroring the existing `?` private-mode guard; a regression test in parser.rs's test module proves it.
- pre-approved-batch: yes

### ligatures-default-off
- declared-surface:
  - crates/common/src/config.rs
- depends_on: []
- accept-seed: With the default config, `font.ligatures` is `false`, so typing `first` renders as five discrete monospace cells with no ghost space; a `common` unit test asserts `FontConfig::default().ligatures == false`.
- pre-approved-batch: yes
