# Domain: terminal vocabulary & invariants

Inferred from `requirements-interview-outputs/mac-terminal/insights.md`.

## Vocabulary

| Term | Meaning here |
|------|--------------|
| **PTY** | Pseudo-terminal pair. We spawn `/bin/zsh` on the slave; read/write the master. `portable_pty`. |
| **Cell** | One grid position: a `char` + fg/bg `Color`. The unit of "manual control". |
| **Grid** | Row-major `cols × rows` cells + cursor. The mutable model VT parsing writes into. |
| **Scrollback** | History above the visible viewport (baseline: viewport scroll only; full scrollback is a feature). |
| **GridSnapshot** | Immutable clone of the visible grid — the contract `renderer` consumes. Never a live `Grid`. |
| **VT / ANSI escape** | Control sequences zsh emits (cursor moves, colors, erase). Parsed by `vte`; we own the semantics. |
| **Glyph atlas** | GPU texture packing rasterized glyphs. `swash` rasterizes; the atlas is a pending feature. |
| **Ligature** | Multi-char glyph (e.g. `!=`, `=>`). Config `font.ligatures`; handled during swash shaping. |
| **IME** | Input Method Editor (e.g. Korean 한글). winit delivers composed text via `KeyEvent::text`. |
| **ai-monitor** | A toggle overlay that watches the active session's logs (`~/.claude/projects/**/*.jsonl`). Native, Rust-rewritten. A future feature. |

## Invariants

1. **Manual cell control.** Rendering is per-cell and hand-rolled on wgpu. No GUI
   toolkit ever owns layout. (`no-gui-crates` rule.)
2. **`term-core` is GPU-free.** The engine never references wgpu/winit, so every
   terminal behavior is testable headlessly. (`dependency-direction` rule.)
3. **macOS + zsh + local only.** No cross-platform, no other shells, no SSH.
   (`macos-only`, `zsh-local-only` rules.)
4. **Performance is judged by numbers.** Candidate budgets (to be finalized before
   they become gates): 120fps sustained, input→screen ≤ 8ms, no frame drops
   scrolling 100k lines. Until locked, treat as goals, not pass/fail gates.

## Deferred (features via `/specify`, not scaffold)

All of the original scaffold-deferred features have since shipped: inline image
paste (Kitty graphics-APC protocol), the ai-monitor overlay, live settings (TOML
config reload — a graphical panel remains a non-goal), full scrollback,
256-color/truecolor + bold/dim/italic/underline/reverse SGR, and glyph atlas
rendering. No scaffold-deferred features remain; new work is scoped via `/specify`
per feature.
