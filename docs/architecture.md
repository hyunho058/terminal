# Architecture decisions & boundaries

The full derivation lives in `scaffold-spec.md`. This is the working summary.

## Decisions (L2)

| # | Decision | Why |
|---|----------|-----|
| D3 | Pure wgpu/Metal, hand-rolled cell rendering, no GUI crates | The whole point — own every cell |
| D4 | `winit` for window/event-loop/IME; `objc2` for macOS-only bits | wgpu-native, IME, low boilerplate |
| D5 | `vte` for escape parsing + our own grid/scrollback | Don't reinvent VT; keep grid control |
| D6 | `swash` shaping + rasterization + our own GPU atlas | Ligatures; GPU cell drawing stays ours |
| D7 | Threads + channels (no async runtime) | PTY reader thread → channel → UI thread |
| D8 | cargo workspace, 4 crates | Enforce dependency direction → headless core |
| D9 | TOML + serde config (`~/.config/mac-terminal/config.toml`) | Standard, human-editable |
| D10 | `cargo test` + `insta` snapshots + headless smoke | Test GPU-app logic without a GPU |
| D11 | GH Actions (fmt/clippy/test/build) + local `.app` | Baseline drift resistance |

## Boundaries

- **`common`** depends on nothing internal. Config/error/logging only. UI/GPU-free.
- **`term-core`** depends on `common` only. Owns `Grid`, `Processor` (vte),
  `Pty`, and `GridSnapshot`. The `GridSnapshot` type is the **versioned contract**
  the renderer reads — extend it deliberately.
- **`renderer`** depends on `common` + `term-core`. Takes a raw-window-handle, not
  a winit `Window` — so it never pulls in winit. One draw call per frame from a
  snapshot.
- **`app`** depends on all three. Owns the winit event loop, the PTY reader
  thread, and input encoding. The only crate that knows about windows.

## Runtime model (C6)

The render loop is **event-driven**, not free-running: winit waits
(`ControlFlow::Wait`). PTY output arrives on the reader thread, goes over an
`mpsc` channel, and wakes the loop via `EventLoopProxy::send_event`. The loop
drains the channel, advances the grid, and requests one redraw.

```
zsh ──PTY──> reader thread ──mpsc──> [wake] winit loop
                                          │ advance(grid)
                                          ▼
                                     term-core Grid ──snapshot──> renderer ──> Metal
```

## Deferred / known gaps

- **G1** Performance numbers unconfirmed — CI checks build/test, not fps/latency.
- **G2** Inline-image protocol undecided (iTerm2 vs Kitty vs custom).
- **G3** Glyph atlas: `swash` rasterizes today; wiring the atlas into the shader
  (replacing block-font cells) is the first render feature.
