---
name: run-term
description: |
  Build and launch the mac-terminal app to eyeball a render or behavior change.
  Use when you changed rendering, input, PTY, or the event loop and want to see it
  running. Triggers: "run the terminal", "launch it", "does it render", "/run-term".
argument-hint: "[--release]"
allowed-tools:
  - Bash
  - Read
---

# /run-term — launch the terminal

Build and run the app binary, capturing startup logs so you can confirm the
`window → Metal surface → zsh → grid` path came up.

## Steps

1. Build:
   ```sh
   cargo build -p app            # add --release for perf checks
   ```
2. Launch in the background with info logging, capture output:
   ```sh
   RUST_LOG=info ./target/debug/app > /tmp/mac-terminal.log 2>&1 &
   ```
   (Use `target/release/app` if you built `--release`.)
3. Read `/tmp/mac-terminal.log` and confirm three lines appear:
   - `gpu adapter selected ... backend=Metal`
   - `surface configured ... format=Bgra8UnormSrgb`
   - `terminal started: zsh spawned, grid ready cols=.. rows=..`
4. It is a GUI app — it will not exit on its own. When done, `kill` the PID.

## Notes

- No adapter / surface error line ⇒ GPU init failed; check the log tail.
- The renderer currently draws non-blank cells as colored blocks (block-font);
  seeing a block pattern that reacts to typing is success until the glyph atlas
  lands.
- Do not add a headless flag to force-exit unless the task calls for it.
