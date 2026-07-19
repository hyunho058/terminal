# Spec: housekeeping

## Meta
- **Created**: 2026-07-18
- **Type**: chore
- **Status**: planned (decompose batch — L2-reviewer PASS; execution owned by build-order)
- **Set**: post-pr2-backlog (`specs/post-pr2-backlog/partition-manifest.md`)

## Goal
Prune dead branches and refresh the stale "deferred / known-gaps" lists so the
docs stop claiming shipped work is deferred. Delete the 3 dead branches
(`build/mac-terminal-v1`, `fix/terminal-rendering` local+remote,
`feat/terminal-core` local), and update `docs/` + `CLAUDE.md` to reflect what PR
#1/#2 and the sibling features (`idle-cpu`, `vt-color-attrs`) actually shipped.

## Non-goals
- Any code change (this feature owns `docs/**` + `CLAUDE.md` only — no `crates/`).
- Broad doc rewrites: only the deferred/known-gaps lists and the block-font
  rendering note are touched, not unrelated prose.
- Deleting `main` or the active `perf/idle-cpu` branch, or any branch with
  unmerged work.
- Confirming the provisional performance *numbers* (the substantive G1 decision is
  a human call; this feature only refreshes the note's wording, not the thresholds).

## Confirmed Goal
The dead branches are pruned **merge-safely by content**: the three were
squash-merged (PR #1 `d6c5e5b`, PR #2 `58162fd`), so their tips are not ancestors
of `main` and `git branch -d` refuses them on reachability — the merge-safety gate
is therefore **content-equivalence** (`git diff main..<b>` empty and no files
unique to the branch), not reachability. A branch whose content is fully in `main`
is deleted (local, plus its `origin` copy); a branch that still carries unique
unmerged content is NOT deleted — it is surfaced to the human. The
deferred/known-gaps lists in `docs/domain.md` and `docs/architecture.md`, the G1
note in `docs/performance.md`, and the "block-font" note in `CLAUDE.md` are
refreshed: items verified shipped at run time (a `specs/<feature>/` dir + wired
code) are struck, genuinely-open items stay, and no item is dropped without that
verification. "256-color/truecolor SGR" is struck only once D5 re-confirms the
sibling `vt-color-attrs` has actually landed its `38;5`/`38;2` arms.

## Declared Surface
- feature-id: housekeeping
- depends_on: [idle-cpu, vt-color-attrs]
- declared-surface:
  - docs/**
  - CLAUDE.md

## Research
- Branches present: local `build/mac-terminal-v1`, `feat/terminal-core`,
  `fix/terminal-rendering`, `main`, `perf/idle-cpu` (current); remotes
  `origin/build/mac-terminal-v1`, `origin/fix/terminal-rendering`, `origin/main`
  (`git branch -a`). So `feat/terminal-core` is local-only; the other two have an
  `origin` copy to delete as well — matching the task's "local+remote / local".
- The three were **squash-merged** (PR #1 `d6c5e5b`, PR #2 `58162fd`), so none is an
  ancestor of `main` (`git branch --merged main` shows only `main` + `perf/idle-cpu`)
  and `git branch -d` refuses all three on reachability. Content reality differs per
  branch: `git diff main..fix/terminal-rendering` is **empty** (tree ≡ main);
  `feat/terminal-core` adds no files main lacks (content subset); but
  `build/mac-terminal-v1` has unique unmerged content (`autopilot.md` etc.). Hence D6
  gates on **content-equivalence**, not reachability.
- `docs/domain.md:32-35` "Deferred" lists: inline image paste, ai-monitor overlay,
  settings UI, full scrollback, **256-color/truecolor SGR**, glyph atlas rendering —
  most already shipped (see below).
- `docs/architecture.md:45-50` "Deferred / known gaps": **G1** perf numbers
  unconfirmed, **G2** inline-image protocol undecided, **G3** glyph atlas not yet
  wired into the shader ("replacing block-font cells").
- `docs/performance.md:3` "STATUS: PROVISIONAL — numbers NOT yet human-confirmed
  (gap G1 open)"; §"Confirming the numbers (the open G1 decision)" at line 84.
- `CLAUDE.md:39-41` "Rendering scope: the current renderer draws non-blank cells as
  colored blocks (block-font). Real glyphs = the swash atlas … the first feature to
  build" — stale: `build_glyph_instances` (`crates/renderer/src/lib.rs:~685`) already
  rasterizes real swash glyphs via `self.atlas`.
- Shipped-work evidence (for verify-before-strike): `specs/{ai-monitor, settings,
  scrollback, image-paste, glyph-rendering}` dirs exist with matching wired code
  (`crates/ai-monitor`, `app/src/settings.rs`, `term-core/src/scrollback.rs`,
  `term-core/src/image.rs` + `renderer/src/image_layer.rs`, `renderer/src/{glyph,
  atlas}.rs`). 256/truecolor SGR ships in the sibling `vt-color-attrs`
  (`handle_sgr` gains the `38;5`/`38;2` arms).
- This feature `depends_on: [idle-cpu, vt-color-attrs]`, so build-order runs it only
  after both pass — the "256/truecolor SGR shipped" and "idle-CPU addressed" claims
  are true at execution time.

## Decisions

<!-- SD1–SD4 inherited verbatim from specs/post-pr2-backlog/partition-manifest.md
     (shared common ground for the whole set; not all are directly actionable in a
     docs/branches chore, but they are inherited unchanged per the T1 contract). -->
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

<!-- feature-local decisions -->
### D5: Verify-before-strike — never drop a still-open item, never keep a shipped one
- **Status**: resolved
- **Rationale**: For each entry in a "Deferred" / "known gaps" list, confirm its real
  state before editing: **shipped** iff a `specs/<feature>/` dir exists AND the code
  is wired (not just scaffolded); otherwise it stays. Shipped items are struck (or
  moved to a short "Shipped since scaffold" note); genuinely-open items (e.g. the
  substantive G1 threshold decision) remain. Rejected: deleting the whole list
  wholesale (would silently drop a still-open item), or leaving it stale (the point
  of the chore).

### D6: Branch deletion is merge-safe by CONTENT-equivalence (squash-merge aware)
- **Status**: resolved
- **Rationale**: The three branches were **squash-merged** into `main`, so their tips
  are not ancestors of `main` and `git branch -d` (a *reachability* check) refuses all
  three even though two are content-identical to `main`. The correct merge-safety gate
  is therefore **content-equivalence**, checked per branch before deletion:
  - **Safe to delete** iff `git diff main..<b>` is empty AND
    `git diff --diff-filter=A main..<b>` lists no files unique to the branch (its
    content is fully in `main`). Only then delete — with `git branch -D` (necessary
    because `-d` refuses the squash-merged tip), justified by the passing content gate,
    plus `git push origin --delete <b>` for a branch with an `origin` copy.
  - **NOT safe → escalate**: if the branch has any unique content (a non-empty diff or
    a file `main` lacks), STOP and surface it to the human — do not delete.
  Rejected: gating on reachability / `git branch -d` alone (refuses all three here, so
  the chore deletes nothing); a blind `git branch -D` with no content check
  (irreversible data-loss on a branch that still holds unmerged notes).

### D7: Tight diff — only the deferred/known-gaps lists + the block-font note
- **Status**: resolved
- **Rationale**: Touch only the stale lists (`domain.md` Deferred, `architecture.md`
  G1–G3, `performance.md` G1 status) and the `CLAUDE.md` "block-font" rendering-scope
  note; leave surrounding prose intact so the change is small and reviewable.
  Rejected: a broad docs rewrite (scope creep beyond the task).

## Constraints
- This feature edits `docs/**` and `CLAUDE.md` only — zero `crates/` changes (its
  declared surface). It must not touch code owned by the sibling features.
- No branch with unmerged work is deleted; `main` and the active branch are never
  touched (D6).
- Every struck deferred item must be verified shipped first (D5); no silent drop of
  a genuinely-open gap.
- The performance *numbers* / thresholds are not changed — only the G1 note wording
  is refreshed to point at the shipped idle-CPU work; the threshold-confirmation
  decision stays a human call.

## Known Gaps
- `build/mac-terminal-v1` carries unique content not in `main` (e.g. `autopilot.md`,
  divergent `build_order.md` / `specs/glyph-rendering/loop.md`), so under D6 it is
  NOT auto-deleted — it is surfaced to the human, who decides whether those notes are
  worth keeping before the branch is removed. The task named 3 branches to delete; one
  turns out not to be fully dead. (State may change by run time; the D6 content gate is
  re-evaluated then.)
- Whether G2 (inline-image protocol) is fully *resolved* vs a "first-cut protocol,
  final choice open" depends on what `image-paste` committed to; the refresh reflects
  the shipped state and leaves the note open if the protocol is genuinely still TBD.
- G1's substantive decision (commit the perf thresholds / machine class) remains a
  human call and is intentionally left open; only the wording is refreshed.

## Requirements

### R0: Docs reflect reality and the dead branches are gone
#### R0.1: A reader sees no shipped feature listed as deferred, and no stale branch
- **Given**: the refreshed docs and the pruned branch list
- **When**: a developer reads `docs/domain.md` / `docs/architecture.md` / `CLAUDE.md`
  and runs `git branch -a`
- **Then**: no already-shipped feature appears in a "Deferred" list, the block-font
  note reflects real-glyph rendering, and none of the 3 dead branches remain

### R1: Prune the dead branches, merge-safe by content
#### R1.1: Content-in-main branches are deleted
- **Given**: a target branch whose content is fully in `main` — `git diff main..<b>`
  is empty AND `git diff --diff-filter=A main..<b>` lists no unique files (at spec time
  this holds for `fix/terminal-rendering` — empty tree diff — and `feat/terminal-core`
  — a strict subset, no files main lacks)
- **When**: the content gate passes and `git branch -D <b>` is run (from a safe
  branch; `-D` is required because the squash-merged tip makes `-d` refuse)
- **Then**: the local branch is removed, the deletion loses no content, and any
  `origin` copy is removed with `git push origin --delete <b>`
#### R1.2: The remote copies follow the local deletion
- **Given**: `origin/build/mac-terminal-v1` and `origin/fix/terminal-rendering` exist
  and there is no `origin/feat/terminal-core` (local-only)
- **When**: a branch is deleted under the R1.1 content gate
- **Then**: its `origin` copy is deleted too when present; `feat/terminal-core` has no
  remote to delete
#### R1.3: A branch with unmerged content halts and is surfaced
- **Given**: a target branch that still carries unique content not in `main` — at spec
  time `build/mac-terminal-v1` has a non-empty diff and files `main` lacks (e.g.
  `autopilot.md`, divergent `build_order.md` / `specs/glyph-rendering/loop.md`)
- **When**: the content gate fails
- **Then**: that branch is NOT deleted — it is surfaced to the human with its unique
  content, and no `-D` is run on it (D6, "never push through the fence")

### R2: Refresh `docs/domain.md` Deferred list (verify-before-strike)
#### R2.1: 256/truecolor SGR is struck only once D5 re-confirms it shipped
- **Given**: `docs/domain.md:35` lists "256-color/truecolor SGR"; at spec-write time
  `handle_sgr` still has ONLY the 16-color arms (the sibling `vt-color-attrs` is an
  unbuilt draft), and build-order runs this feature only after `vt-color-attrs`'s
  gates pass (`depends_on`)
- **When**: the Deferred list is refreshed AND D5 re-verification at run time confirms
  `handle_sgr` now carries the `38;5`/`38;2` arms
- **Then**: "256-color/truecolor SGR" is removed from the deferred list; if that
  re-verification fails (the arms are absent), the item is LEFT listed and the
  discrepancy is surfaced — the strike is never made on the assumption alone
#### R2.2: Other shipped items are struck, open items kept
- **Given**: the list also names ai-monitor, settings UI, full scrollback, inline
  image paste, glyph atlas rendering
- **When**: each is checked against its `specs/<feature>/` dir + wired code (D5)
- **Then**: those verified shipped are struck (or moved to a "shipped since scaffold"
  line) and any item not yet shipped remains listed

### R3: Refresh `docs/architecture.md` gaps + `docs/performance.md` G1 note
#### R3.1: G3 (glyph atlas) reflects shipped real-glyph rendering
- **Given**: G3 says the atlas is not yet wired into the shader
- **When**: verified against `build_glyph_instances` using the atlas
- **Then**: G3 is updated to "shipped" (glyphs render via the swash atlas), not
  "deferred / first render feature"
#### R3.2: G1 note points at the idle-CPU work without changing thresholds
- **Given**: G1 in `architecture.md` and the `performance.md` G1 status note
- **When**: the note is refreshed after the sibling `idle-cpu` shipped
- **Then**: the wording reflects that idle-CPU was addressed while leaving the
  threshold-confirmation decision explicitly open (numbers unchanged)
#### R3.3: G2 reflects the shipped image-paste state
- **Given**: G2 marks the inline-image protocol "undecided"
- **When**: checked against the shipped `image-paste`
- **Then**: G2 is updated to the protocol actually shipped, or left open with a note
  if the final choice is genuinely still pending (D5)

### R4: Fix the `CLAUDE.md` block-font rendering note
#### R4.1: The rendering-scope note describes real glyphs
- **Given**: `CLAUDE.md:39-41` says the renderer draws "colored blocks (block-font)"
  and calls the swash atlas "the first feature to build"
- **When**: verified against the shipped glyph renderer
- **Then**: the note is updated to state the renderer draws real swash-atlas glyphs
  (block-font is no longer the current behavior)

### R5: No deferred item is dropped without verification
#### R5.1: A still-open gap survives the refresh
- **Given**: a deferred/known-gap entry whose feature is NOT shipped (no spec dir or
  unwired code)
- **When**: the lists are refreshed
- **Then**: that entry remains in the list (the refresh strikes only verified-shipped
  items — D5)

## Tasks

### T1: Prune the dead branches, merge-safe by content [ops]
- **Fulfills**: R0, R1
- **Depends on**: (none)   ← parallel with T2 (git state vs file edits, no overlap)
- From a safe branch, for each of `build/mac-terminal-v1`, `feat/terminal-core`,
  `fix/terminal-rendering` run the D6 content gate (`git diff main..<b>` empty AND no
  files unique to the branch). Delete the passing ones with `git branch -D <b>` (the
  squash-merged tip makes `-d` refuse) plus `git push origin --delete <b>` where an
  `origin` copy exists. For any branch that fails the gate (expected:
  `build/mac-terminal-v1`), STOP and surface its unique content to the human — do not
  delete (R1.3). Confirm final state with `git branch -a`.

### T2: Refresh the deferred/known-gaps lists + block-font note [docs]
- **Fulfills**: R0, R2, R3, R4, R5
- **Depends on**: (none)   ← parallel with T1 (disjoint: `docs/**` + `CLAUDE.md`);
  the whole feature `depends_on: [idle-cpu, vt-color-attrs]` per the manifest, so
  build-order runs it only after both siblings pass — making the "shipped" claims true
- Verify each deferred/known-gap item against its `specs/<feature>/` dir + wired code
  (D5), then strike the shipped ones (incl. 256/truecolor SGR) from `docs/domain.md`,
  update G1/G2/G3 in `docs/architecture.md` and the G1 note in `docs/performance.md`,
  and fix the block-font note in `CLAUDE.md`. Keep any unverified item listed (R5) and
  leave surrounding prose untouched (D7).

## External Dependencies

### Pre-work
- (none) — build-order guarantees the sibling features have passed before this runs.

### Post-work
- (none) — `git branch -a` and a docs re-read are the in-task acceptance checks.
