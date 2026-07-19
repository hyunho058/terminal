# mac-terminal

A from-scratch macOS GPU terminal in Rust: pure **wgpu/Metal** cell rendering (no
GUI/widget crates), local **zsh** over a PTY, VT parsing via `vte`. macOS ARM64
only. Daily-driver replacement for Warp/iTerm2.

## Workspace layout

```
crates/
  common/     # config (TOML+serde), error, logging — no GPU/UI deps
  term-core/  # PTY, vte parsing, cell grid, scrollback, GridSnapshot — GPU-free
  renderer/   # wgpu/Metal cell renderer + swash glyph path — window-agnostic
  app/        # winit event loop; wires the three together (the binary)
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

## Context (loaded on demand)
@docs/domain.md        # terminal vocabulary + invariants
@docs/architecture.md  # the L2 decisions, boundaries, and what's deferred

## Conventions

- **Build/test/lint:** `cargo build`, `cargo test --workspace`, `cargo clippy
  --workspace --all-targets -- -D warnings`. Toolchain pinned in `rust-toolchain.toml`.
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
