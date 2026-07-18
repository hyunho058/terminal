# Rule: dependency direction (keep the core headless)

**Constraint C4.** The workspace dependency graph is one-directional:

```
common  ←  term-core  ←  renderer  ←  app
```

## The hard invariant

**`term-core` must never depend on `wgpu` or `winit`** (directly or transitively
through a new crate). The terminal engine — grid, vte parsing, PTY, scrollback —
stays GPU-free and window-free so it is testable without a display. The renderer
only ever sees `term_core::GridSnapshot`, never a live `Grid`.

## When adding a dependency or module, ask

- Does this pull a GPU/windowing type into `term-core`? → **No.** Put it in
  `renderer` or `app`, and pass data across via `GridSnapshot` (extend that type
  if needed).
- Does `renderer` now import `winit`? → **No.** Keep it window-agnostic via
  `raw-window-handle`; the `app` crate owns the window.
- Does `common` depend on any sibling? → **No.** It sits at the bottom.

If a change seems to require breaking this direction, that is a design decision
for the human — surface it, don't route around it.
