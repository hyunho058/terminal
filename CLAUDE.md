# Crewterm

(Formerly `mac-terminal`; specs and interview notes written before 2026-11-02 use
the old name. The config dir, shell-integration dir and env vars carry the new one,
and `Config::path` still reads a config left under the old dir.)

A from-scratch macOS GPU terminal in Rust: pure **wgpu/Metal** cell rendering (no
GUI/widget crates), local **zsh** over a PTY, VT parsing via `vte`. macOS ARM64
only. Daily-driver replacement for Warp/iTerm2.

## Workspace layout

```
crates/
  common/     # config (TOML+serde), error, logging — no GPU/UI deps
  term-core/  # PTY, vte parsing, cell grid, scrollback, GridSnapshot — GPU-free
  renderer/   # wgpu/Metal cell renderer + swash glyph path — window-agnostic
  app/        # winit event loop; wires the three together (lib + thin bin)
docs/         # loaded on demand (below)
```

**Dependency direction (enforced):** `common ← term-core ← renderer ← app`.
`term-core` MUST NOT depend on wgpu or winit — that is what keeps the terminal
logic headless-testable. See `.claude/rules/dependency-direction.md`.

## The exemplar

`crates/app/src/terminal_app.rs` is the **vertical-slice reference**: window →
GPU surface → spawn zsh → PTY reader thread → channel → `EventLoopProxy` wake →
`term-core` grid → `renderer` draw → keyboard/resize. Copy this flow's shape when
adding features. `crates/term-core/tests/grid_snapshot.rs` is the test exemplar
(insta snapshots of the `vte → grid` pipeline, no GPU).

`crates/renderer/tests/golden_frame.rs` is the exemplar for the **other** half —
the one the snapshot tests structurally cannot reach. It renders through
`Renderer::capture` and asserts the pixels that came back, because every other
renderer observable reports a *decision* (`reshape_count`, `Atlas::generation`)
and stays correct while the frame it describes is blank: zeroing the glyph
instance count left the whole suite at 1460 passed / 0 failed. A change to the
render path is not covered until something asserts what reached the screen.

## Context (loaded on demand)
@docs/domain.md        # terminal vocabulary + invariants
@docs/architecture.md  # the L2 decisions, boundaries, and what's deferred

## Conventions

- **Build/test/lint:** `cargo build`, `cargo test --workspace`, `cargo clippy
  --workspace --all-targets -- -D warnings`. Toolchain pinned in `rust-toolchain.toml`.
- **Coverage is judged by mutation, not by count.** `cargo mutants -f <file> -j 4`
  reports which functions can be broken with the suite still green — the only
  check that a test *protects* rather than merely executes. Config and the
  scoping rules are in `.cargo/mutants.toml`; run it scoped (the workspace has
  ~6300 mutants), and treat a survivor as a missing test unless it is provably
  equivalent. Precedent, with 1460 tests passing: `session.rs` left **16
  survivors** — the whole tab-title precedence machine — and `tabs.rs` **41**,
  where `next`, `prev`, `jump` and `resize_all` could each be replaced by a
  no-op. Both are at 0 now. Record the survivor count, not a ratio: a file's
  mutant *total* moves with every edit to it, so a ratio written down here is
  stale by the next commit — which is how three different totals for the same
  `session.rs` run (36, 39, 35) came to sit in this repo at once. Run
  `cargo mutants --list -f <file>` for the total as it stands today.
  An *equivalent* survivor is usually a sign the condition says less than it
  means — prefer rewriting it (`n < size` → `n != size`) over adding an
  `exclude_re` entry, which is unanchored by default and silences every
  same-named function in the workspace.
- **Errors:** per-crate `Error` enums via `thiserror`; no `unwrap()` outside tests.
- **Rendering scope:** the renderer draws real glyphs via the swash atlas
  (`renderer/src/glyph.rs` + `renderer/src/atlas.rs`), with per-cell fg/bg,
  256-color/truecolor, and bold/dim/italic/underline/reverse attributes. (The
  original block-font placeholder has been replaced.)
- Solo project — no commit/branch conventions enforced.

## Project skills

- `/run-term` — build & launch the terminal to eyeball a render change
- `/insta-review` — review/accept snapshot diffs after grid/parser changes

## Active hooks (`.claude/settings.json`)

- **PostToolUse:** `rustfmt` on any edited `*.rs`
- **PreToolUse:** block edits to `Cargo.lock` (let cargo manage it)

## Rules (`.claude/rules/`)

`no-gui-crates` · `macos-only` · `dependency-direction` · `zsh-local-only` —
read before adding dependencies or `#[cfg]` branches.
