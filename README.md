# Crewterm

A from-scratch GPU terminal for macOS, written in Rust, built as a daily driver for
working alongside AI coding agents.

- **Hand-rolled rendering.** Every cell is drawn directly with wgpu on Metal. No GUI or
  widget toolkit owns any part of the layout.
- **Local zsh over a PTY.** VT parsing uses `vte`; the grid, scrollback and semantics
  are this project's own.
- **Agent-aware chrome.** A tab sidebar, a docked AI monitor and a full-window Agent
  Monitor show what each Claude Code, Antigravity, Codex, Gemini or Aider session is
  doing, and which ones are waiting on you.

> **Platform:** macOS on Apple Silicon (`aarch64-apple-darwin`) only, and `/bin/zsh`
> only. Cross-platform support, other shells and SSH are deliberate non-goals.

*(Formerly `mac-terminal`. Older specs use that name.)*

---

## Features

### Terminal
- Glyph rendering through a swash-rasterized GPU atlas, with font fallback (Menlo →
  Apple SD Gothic Neo for Hangul), **real bold/italic faces**, procedural box drawing,
  wide characters and optional programming ligatures.
- 256-color and truecolor, plus bold / dim / italic / underline / reverse SGR.
- TUI support: alternate screen, cursor-key modes, mouse button/drag reporting and a
  blinking cursor.
- Scrollback of 100,000 lines, mouse selection, and copy/paste (Cmd+C / Cmd+V).
- **Image paste:** pasting an image from the clipboard writes a PNG and types its path
  into the shell, so agent CLIs receive a real file.
- **Region capture** (Cmd+Shift+G): drag a rectangle; the PNG's path is typed into the
  tab that started the capture.
- Inline images over the **Kitty graphics protocol**.
- **Hyperlinks:** hold ⌘ to highlight a URL under the pointer, then click to open it.
- **Korean IME:** composition preedit, and Cmd chords that keep working under the 2-Set
  Korean input source.
- zsh shell integration (OSC 133) through a private `ZDOTDIR`. Your own startup files
  run first, so oh-my-zsh, starship and powerlevel10k keep working.

### Tabs and agent awareness
- **Left tab sidebar** with one card per tab: title, cwd, git branch and agent status
  (`Working`, `Waiting for input`, …). Cards come from shell and tmux pane state, and a
  separate band lists the tabs waiting on you. The sidebar is drag-resizable and
  collapsible, and tabs can be renamed.
- **AI monitor** (Cmd+Shift+M): a right-docked, drag-resizable panel that reads the
  active tab's Claude Code / Antigravity session log live. It shows context usage,
  tools, subagents and activity. The panel reserves terminal columns rather than
  covering them.
- **Agent Monitor overview** (Cmd+Shift+F): a full-window overlay with one row per agent
  session in the window. Rows are grouped as *Needs you / Working / Idle*, and each one
  shows project, branch, current activity, subagents and elapsed time. Open a row for a
  per-session timeline, or jump straight to its tab.

### Settings
- Native settings window (Cmd+, or *Crewterm → Settings…*) with Font & Grid, Theme &
  Colors and Sidebar & AI Monitor tabs.
- All settings live in a TOML file that is **live-reloaded**: edit it and the change
  takes effect without a restart.

---

## Install and run

Requirements: an Apple Silicon Mac and a stable Rust toolchain via `rustup`. The
toolchain components come from `rust-toolchain.toml`.

```sh
# Run from source
cargo run -p app --release          # binary: target/release/crewterm

# Build a local .app bundle (unsigned, for the machine that built it)
scripts/bundle-app.sh               # → target/Crewterm.app
open target/Crewterm.app
```

To see logs, run the binary directly and set `RUST_LOG`, e.g. `RUST_LOG=info
target/release/crewterm`. Logs go to **stdout**.

---

## Keyboard shortcuts

| Chord | Action |
|-------|--------|
| Cmd+T / Cmd+W | New tab / close tab |
| Cmd+Shift+] / Cmd+Shift+[ | Next / previous tab |
| Cmd+1 … Cmd+9 | Jump to tab *n* |
| Cmd+Shift+R | Rename the active tab |
| Cmd+Shift+S | Collapse / expand the sidebar |
| Cmd+Shift+M | Toggle the AI monitor panel |
| Cmd+Opt+1 … Cmd+Opt+6 | Jump to a section of the AI monitor |
| Cmd+Shift+F | Toggle the Agent Monitor overview (`Esc` goes back / closes) |
| Cmd+Shift+G | Capture a screen region and type its path |
| Cmd+C / Cmd+V | Copy selection / paste text or an image |
| Cmd+= / Cmd+- / Cmd+0 | Font size up / down / reset |
| Cmd+, | Open Settings |
| ⌘ + hover, click | Highlight and open a link |

---

## Configuration

Path: `~/Library/Application Support/Crewterm/config.toml`

Every field is optional. Unknown keys are **rejected**, so a typo fails loudly instead
of being ignored. Out-of-range widths and font sizes are clamped with a warning. If no
file exists, *Settings* seeds one from [`config.example.toml`](config.example.toml).

```toml
[font]
family = "Menlo"
size_pt = 14.0          # clamped to 8–32
ligatures = false       # off by default: Menlo's fi/fl ligatures collapse cells

[theme]
background = 0x0F1419   # packed 0xRRGGBB
foreground = 0xE6E1CF

[grid]
columns = 100           # initial hint; the grid is fit to the window
rows = 30

[sidebar]
width_px = 340.0        # clamped to 260–440
show_git_branch = true
show_agent_status = true
collapsed = false

[ai_monitor]
width_px = 430.0
open = false
# context_window = 1000000  # tokens; overrides the per-model table (e.g. 1M sessions)
```

The sidebar width, its collapsed state, the monitor's width and open state, and the font
size are written back to this file when you change them in the UI. Comments and the
other keys are preserved.

---

## Architecture

A Cargo workspace whose dependency direction is enforced:

```
common  ←  term-core  ←  renderer  ←  app
                                       ↑
                        ai-monitor ────┘
```

| Crate | Role |
|-------|------|
| `common` | Config (TOML + serde), config watcher, errors, logging. No GPU/UI deps. |
| `term-core` | PTY, `vte` parsing, cell grid, scrollback, selection, links, Kitty images, shell integration, and the `GridSnapshot` contract. **GPU-free and window-free.** |
| `ai-monitor` | Headless, read-only reader of Claude Code / Antigravity session logs: activity state, telemetry, session digests. |
| `renderer` | wgpu/Metal cell renderer, swash glyph atlas, and every drawn layer (sidebar, monitor panel, overview, selection, cursor, images). Window-agnostic: it takes a `raw-window-handle`, not a winit window. |
| `app` | winit event loop, input encoding, tabs, the tab-metadata worker, overlays, and the native AppKit menu and settings window. A library plus a thin `main.rs`. |
| `perf` | Owns the Criterion benches in `benches/`. |

The runtime is threads and channels, with no async runtime:

```
zsh ──PTY──> reader thread ──mpsc──> [wake] winit loop ──> Grid ──snapshot──> renderer ──> Metal
```

Further reading:
- [`docs/architecture.md`](docs/architecture.md): decisions, boundaries, known gaps
- [`docs/domain.md`](docs/domain.md): vocabulary and invariants
- [`docs/performance.md`](docs/performance.md): performance targets
- [`specs/`](specs/): one spec per shipped feature

---

## Development

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy, tests and a release build on a
macOS runner.

- **Snapshot tests:** `term-core` checks the `vte → grid` pipeline with `insta`
  snapshots (`crates/term-core/tests/grid_snapshot.rs`).
- **Pixel tests:** `crates/renderer/tests/golden_frame.rs` renders through
  `Renderer::capture` and asserts on the captured frame. A render-path change needs
  this kind of test, because the other renderer observables stay green on a blank frame.
- **Mutation testing:** test quality is judged by surviving mutants, not by test count.
  Run it scoped to one file: `cargo mutants -f <file> -j 4` (config in
  `.cargo/mutants.toml`).
- **Benchmarks:** `cargo bench -p perf` covers input→grid, frame submit and 100k-line
  scroll. The targets in `docs/performance.md` are still **provisional**.
- **Idle CPU:** `scripts/measure-idle-cpu.sh`. The window redraws continuously while it
  is visible, by design (see *Runtime model* in `docs/architecture.md`).

`CLAUDE.md` and `.claude/rules/` hold the project rules: no GUI crates, macOS only,
dependency direction, zsh local only.

---

## Known limitations

- **Region capture needs a Screen Recording grant, and an ad-hoc-signed rebuild voids
  it.** Re-grant after each `cargo build --release`. Alternatively, use Cmd+Shift+4,
  then Cmd+V in Crewterm, which reaches the same end state.
- Agent status relies on other tools' log formats and dialog wording, which this repo
  cannot pin. When an agent CLI changes those, a card can quietly fall back to a less
  precise state. `docs/architecture.md` (G8) lists each dependency and how to diagnose
  it.
- Subagents, elapsed time and timelines exist only for Claude Code and Antigravity.
  Codex, Gemini and Aider sessions get status only.

## License

MIT. See [`LICENSE`](LICENSE).
