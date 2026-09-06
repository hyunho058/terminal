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
  It is a **library plus a thin `main.rs`**, not a bare binary: a `[[bin]]`-only
  crate has no importable surface, so `crates/app/tests/` could not exist and no
  test could reach the assembled path — only the pure decision functions each
  module happens to expose. What stays in `main.rs` is what a library cannot own:
  the `EventLoop` (macOS builds one only on the main thread) and the
  process-global `tabmeta` worker. The registration set moved out to
  `feature_overlays`/`feature_handlers` so it is a value a test can assert.
  Consequently `Session::spawn` takes a `session::Wake` callback rather than an
  `EventLoopProxy` — the same move the renderer makes with `raw-window-handle`,
  one layer up: it keeps the whole input→shell→grid path constructible without an
  event loop, which is what `crates/app/tests/vertical_slice.rs` needs. `main` is
  the single place the proxy becomes a `Wake`.

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

- **G4** The ai-monitor is a **right-docked panel**, not an overlay: it reserves columns
  via `Renderer::right_inset` (the symmetric twin of `left_inset`), so opening it reflows the
  terminal rather than covering it. `Grid::resize` has no rewrap, so that reflow currently
  truncates rows wider than the narrowed grid — non-destructive reflow is specified separately
  in `specs/grid-reflow/spec.md` and is a declared dependency of the panel. The panel's list
  area is now a **scroll viewport** (`specs/ai-monitor-panel-scroll`): the six sections are
  fully expanded into one document, reached by wheel or by `Cmd+Opt+1`..`6`, under a sticky
  section heading. Two earlier decisions were reversed there deliberately, and each reversal is
  argued at its site — the panel now consumes keys other than its toggle (`overlay.rs`), and
  its footer draws a chord hint while the new-items badge is up (`monitor_layout.rs`, R1.4).
  One limit is recorded rather than fixed: a rung taller than the content band is unreachable,
  which C1's no-scissor rule forces and which the allocator it replaced shared.
  A later increment put a **fold** back on top of that document: a section with more than
  `FOLD_THRESHOLD` (5) entries draws at most five plus an `ItemKind::Fold` control rung, and
  clicking a heading or that control toggles it. Two things about it are deliberate. It is
  **not** `+N more` returning — that mark reported rows the allocator had already discarded,
  with no way to reach them, whereas this hides rows the document still holds and the control
  is the way back. And the fold state lives **inside** `MonitorModel` precisely where the
  scroll offset is kept out of it: a fold changes which rungs exist, so it must invalidate
  stage one, while a wheel notch must not. The click also reverses D10's "no hit-test, no
  focus" for the panel — the reversal is argued at `MonitorHandle::press`, and is bounded by
  the panel resolving the point against its own published bands so the app still performs no
  geometry.
- **G1** Performance *numbers* still unconfirmed — CI checks build/test, not
  fps/latency; committing the thresholds remains a human decision. (The idle-CPU
  debt from PR #2 — a full re-shape every frame at idle — has since been fixed: the
  renderer skips re-shape on an unchanged snapshot, re-presenting cached buffers. The
  ai-monitor panel carries its **own** shaping cache, and since `specs/ai-monitor-panel-scroll`
  that cache is **two-stage**: stage one lays the whole (now fully expanded) document out
  position-independently, keyed on the model, panel rect, cell metrics and atlas identity;
  stage two translates, culls by ladder index, copies the sticky heading and synthesises the
  footer line, keyed on the scroll state. The split is the contract — the scroll offset is
  deliberately **outside** `MonitorModel` so a wheel notch cannot invalidate stage one, and
  equally deliberately **inside** stage two's key so the screen actually updates. Doing only
  one of the two gives, respectively, this debt back and a panel that never repaints.)
- **G5** The ai-monitor panel derives its log directory from the chosen `claude` process's own
  cwd, while the sidebar card shows the tab's (or tmux pane's) **shell** cwd. After a `cd` inside
  a pane the two legitimately **disagree** — they answer "where is this session's log" and "where
  is this tab working", and forcing agreement would make one of them report something untrue. Not
  a defect; see `specs/tabsession-pane-cwd-gate/spec.md` D11. Two assumptions ride with that
  design, both named rather than assumed silently: **A1**, that `claude` writes to
  `encode_cwd` of the directory it runs in (the kernel-vs-registry cross-check in
  `tabsession::resolve_hint_cwd` is its only detector, and it only fires when a registry record
  exists); and **A2**, that a `claude` process's cwd is fixed at launch (`process.chdir()` would
  make the directory stale with no trigger, since the pid does not change).
- **G2** ~~Inline-image protocol undecided~~ — **resolved**: the Kitty graphics-APC
  protocol shipped (`term-core::image::parse_graphics_apc`).
- **G3** ~~Glyph atlas not yet wired into the shader~~ — **resolved**: glyphs render
  through the swash atlas (`build_glyph_instances` feeds `renderer::atlas::Atlas`),
  with 256-color/truecolor and bold/dim/italic/underline/reverse; block-font is no
  longer the rendering path.
