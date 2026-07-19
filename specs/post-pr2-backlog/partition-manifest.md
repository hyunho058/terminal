# partition-manifest.md — post-pr2-backlog
- generated_at: 2026-07-18
- goal: Clear the PR #2 follow-up backlog for mac-terminal — cut idle CPU by
  skipping per-frame re-shape, close the VT color/attribute gaps so real TUIs
  render correctly, and refresh the stale docs / prune dead branches.

## Shared Decisions
### SD1: Preserve the enforced crate dependency direction; `term-core` stays GPU/winit-free
- **Status**: resolved
- **Rationale**: The workspace graph `common ← term-core ← renderer ← app` is a hard
  invariant (`.claude/rules/dependency-direction.md`). New per-cell data (VT color +
  attributes) lives in `term-core` (`Cell`/`Color`/`parser`); the renderer only ever
  reads it through `GridSnapshot`. Rejected: pushing any wgpu/winit type into
  `term-core`, or letting the renderer see a live `Grid` — either would make the
  engine un-testable headlessly and break the rule.

### SD2: `GridSnapshot` is the versioned renderer contract AND the frame-equality basis
- **Status**: resolved
- **Rationale**: `GridSnapshot` already derives `PartialEq` and is the immutable
  contract the renderer consumes (never a live `Grid`). The idle-CPU fix keys its
  skip-reshape cache on **snapshot equality**; VT attributes are added as `Cell`
  fields, so `Cell: PartialEq`/`Eq` makes attribute-only changes invalidate that
  cache automatically. This is the load-bearing coordination point between the two
  code features. Rejected: a side-channel dirty flag in `app` (splits the source of
  truth from the snapshot), or comparing rendered text (misses color/attr-only
  changes).

### SD3: No GUI/widget crates, macOS-ARM64-only; attribute rendering is hand-rolled via `swash`
- **Status**: resolved
- **Rationale**: `no-gui-crates` + `macos-only` rules. bold/dim/italic/underline/
  reverse are drawn on the existing hand-rolled two-pass cell renderer — synthesized
  through `swash` (embolden/skew) or font faces, with underline/reverse composed in
  the instance builders. Rejected: any widget/text-layout toolkit (egui/iced/…) and
  any `#[cfg(target_os = "…")]` portability branch.

### SD4: Keep the continuous-redraw loop (winit#2640); cut idle CPU by making each idle frame cheap
- **Status**: resolved
- **Rationale**: On this wgpu/winit/macOS stack the compositor drops the presented
  frame the moment the run loop sleeps, so a frame must always be in flight (see the
  `RedrawRequested` comment in `terminal_app.rs`). The idle-CPU fix therefore keeps
  the continuous redraw and makes each idle frame cheap (skip re-shape, re-present
  cached buffers) — it does **not** disable redraws. VT-attribute work must not
  reintroduce unconditional per-frame reshaping. Rejected: on-demand / `ControlFlow::
  Wait`-only redraws (blank window) or dropping `request_redraw` (regresses #2640).

## Features  (ordered — a dependency appears before its dependents)
### idle-cpu
- declared-surface:
  - crates/renderer/src/lib.rs
  - crates/app/src/terminal_app.rs
- depends_on: []
- accept-seed: An idle terminal (no PTY output) drops from ~65% CPU toward idle — `render()` skips re-shaping the grid and re-presents cached buffers whenever the `GridSnapshot` is unchanged, while the continuous-redraw loop keeps a frame in flight.
- pre-approved-batch: yes

### vt-color-attrs
- declared-surface:
  - crates/term-core/src/parser.rs
  - crates/term-core/src/grid.rs
  - crates/renderer/src/lib.rs
  - crates/renderer/src/glyph.rs
  - crates/renderer/src/atlas.rs
- depends_on: [idle-cpu]
- accept-seed: vim / bat / `ls --color` render correctly — SGR parses 256-color (`38;5`/`48;5`), truecolor (`38;2`/`48;2`), bold/dim/italic/underline/reverse, and `39`/`49` default-reset, and the renderer displays those attributes.
- pre-approved-batch: yes

### housekeeping
- declared-surface:
  - docs/**
  - CLAUDE.md
- depends_on: [idle-cpu, vt-color-attrs]
- accept-seed: The 3 dead branches (build/mac-terminal-v1, fix/terminal-rendering local+remote, feat/terminal-core local) are deleted, and the docs' deferred / known-gaps lists no longer name completed items (256/truecolor SGR, the G1 idle-perf debt, the block-font rendering note).
- pre-approved-batch: yes
