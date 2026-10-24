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
| **ai-monitor** | A right-docked, drag-resizable panel (`Cmd+Shift+M`) that watches the active tab's session logs (`~/.claude/projects/**/*.jsonl`). It **reserves** terminal columns rather than covering them. Native, Rust-rewritten. Shipped. |
| **item ladder** | The ai-monitor panel's scroll coordinate space: one rung per heading, card, row or bar, enumerated once by `monitor_layout::item_ladder`. The scroll offset is a rung **index**, not a pixel offset — the panel has no scissor rect, so a half-drawn row has no representation. |
| **fold** | A section with more than five entries draws five plus a control rung (`▸ N more` / `▾ collapse`); clicking its heading or that control toggles it. Distinct from the dead `+N more`, which reported rows that had already been thrown away. The state lives in `MonitorModel` because it changes which rungs exist — unlike the scroll offset, which is kept out of the model for the opposite reason. |
| **sticky slot** | The one heading line reserved at the top of the panel's list area. Content starts *below* it, which is what makes it un-overdrawable: shapes composite under glyphs, so a heading could not otherwise hide rows sliding beneath it. |
| **right dock** | The band at the window's right edge that `Renderer::right_inset` reserves. The ai-monitor's alone: it opens, closes and persists there exactly as on `main` (`AppRequest::SetMonitorOpen`, `OverlayCommand::RestoreMonitor`). The session graph does not share it — it covers it (session-flow-graph D4). |
| **session graph** | The session flow viewer, as a graph (`Cmd+Shift+F`): a **full-window** overlay drawn over the terminal, sidebar and an open ai-monitor without reflowing anything. Nodes are the sidebar's own session cards in three columns joined by curved edges — **project → session → subagent** — under a `{w} working · {q} waiting / {n}` header. A session is an **agent session**: a tab or pane running any detected agent (Claude, Antigravity, Codex, Gemini, Aider), and `n` counts them all; subagents (column 2) exist only under Claude sessions, since only Claude's log records them. The header reads `resolving…` until the worker has resolved every tab's repository; once resolved with no session, a muted line beneath it says there are no agent sessions in this window (the graph never counts other windows). A session's card is the sidebar card's, through the card's own `row_meta` join. While open it owns every key (only `Esc` and the chord act), press, wheel and hover; clicking a session or subagent card switches to its tab and closes it, clicking `N finished` folds that session's finished subagents. Never restored open. Shipped. |
| **project card** | A session graph's column-0 card for one git repository, grouped by the repository's **common dir** (`RepoId.common`), titled with the main worktree's directory name. Present whenever the repository has an agent session in this window, even one running only in a linked worktree; never counted, never clickable. |
| **repo identity** | `RepoId { common, main_root, linked }`, resolved by the tabmeta worker from the **detected agent process's** cwd, any kind (not the shell's, G5) and published only while the graph is open. `linked` puts a session after its repository's main-worktree sessions; no repository puts it in the unparented tail. |
| **slot** | The session graph's scroll unit: one card-height row. A session takes as many slots as it has column-2 nodes (at least one), a project the sum of its sessions'. The wheel moves whole slots, like the ai-monitor's rungs, because the shape path has no scissor. |

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
