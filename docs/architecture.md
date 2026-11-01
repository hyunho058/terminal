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
  event loop, which is what `crates/app/tests/vertical_slice.rs` needs. The proxy
  becomes a `Wake` in exactly one place, `App::start` (`terminal_app.rs`), because
  that is the first moment a window exists and so the first moment there is a grid
  size to spawn a session against. `main` only creates the proxy and hands it over.

## Runtime model (C6)

Work is **event-driven**: winit waits (`ControlFlow::Wait`), set exactly once, in
`main.rs`. PTY output arrives on the reader thread, goes over an `mpsc` channel, and
wakes the loop via `EventLoopProxy::send_event`. The loop drains the channel, advances
the grid, and requests one redraw.

```
zsh ──PTY──> reader thread ──mpsc──> [wake] winit loop
                                          │ advance(grid)
                                          ▼
                                     term-core Grid ──snapshot──> renderer ──> Metal
```

**Presentation is not, and this used to say it was.** An earlier version of this
paragraph read "the render loop is event-driven, **not free-running**". That is true of
the control flow and false of what the app does: while the window is visible it draws
**60 frames a second, forever**, because `WindowEvent::RedrawRequested` ends by
requesting the next redraw (`terminal_app.rs`, the arm's last statement). The loop never
sleeps to idle; it is woken by its own redraw request, measured at
`cause:WaitCancelled=60 win:RedrawRequested=60` per second on an idle window.

That is deliberate and it is load-bearing twice over, which is why the fix is to
describe it rather than to remove it:

1. **winit#2640.** On this macOS/winit-0.30/wgpu stack the compositor drops the
   presented frame as soon as the run loop sleeps, so the window holds content only
   while it is actively drawing. Stop the continuous redraw and an idle window goes
   blank. Four targeted alternatives were tried and failed, `pre_present_notify`,
   `presentsWithTransaction`, a commit-cycle fix, and a buffer refill.
2. **Atlas eviction (D17).** `Renderer::draw_to` clears `last_snapshot` when the glyph
   atlas `generation` bumps, and never asks for a frame itself. It relies on this loop
   to repaint. Reverting to on-demand redraws silently reopens a permanent stale-glyph
   corruption path.

**What it costs, measured rather than assumed**, on an idle window with nothing on
screen changing:

| build | CPU at idle |
|-------|-------------|
| `--release` | ~7% of one core |
| debug | ~19% of one core |

The debug premium is `objc2` verifying Objective-C method signatures on every message
send, which at 60fps is a great many sends. It is a development-time cost only.

Occlusion is the escape hatch and it is honoured: the redraw request is gated on
`!inner.occluded`, so a hidden or fully covered window costs nothing. **There is no such
escape for a visible idle window**, and that is the honest statement of this project's
idle behaviour. See G1 for what the caches do and do not buy against it.

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
  fps/latency; committing the thresholds remains a human decision. **And the one idle
  number that has now been measured is not zero: a visible idle window costs ~7% of a
  core in release, ~19% in debug.** That is the price of C6's continuous redraw, which
  exists so the window does not go blank; read this gap together with that section,
  because "the idle-CPU debt has been fixed" below is easy to read as "idle is free" and
  it never meant that. What was fixed is the **per-frame work**, not the frames. The
  frames are unconditional while the window is visible, and no cache can remove them.
  Nothing currently guards this number: no test, no CI check, no budget. A change that
  doubled it would be invisible.
  (The idle-CPU
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
- **G6** The CONTEXT tile's denominator is now READ rather than guessed, and the read hangs on
  one string of someone else's UI. `specs/ai-monitor-context-window` establishes that a session
  log states its context window in exactly one place — the stdout of `/context` — and nowhere as
  a field: no numeric `200000|700000|1000000` field exists anywhere in the corpus, and every
  `[1m]` marker is prose inside a message. So `telemetry::on_system` gates on the panel's own
  title (`Context Usage`) and reads two anchors inside it. Recorded here, beside G5's A1/A2,
  because it is the same *kind* of fact as those: an assumption about an external tool's output
  that this repo cannot enforce and would not be told about. If Claude Code renames that title,
  the read and **both** its diagnostic counters die together, and the panel falls back to today's
  table-plus-promotion behaviour with nothing to say about it. Two counters reading 0 for a long
  time is the signal to suspect the gate first. The fallback is byte-identical to the pre-feature
  path, which is why the failure is quiet rather than wrong — and why it needs to be written down.
- **G7** A Screen Recording (TCC) grant **does not survive a rebuild**, so region-capture's
  chord stops working after every `cargo build --release` until the user re-grants.
  Measured 2026-09-24 on macOS 26.6.2 (25G83), a clean before/after with one variable:
  the bundle built at 10:27 was granted and captured fine; the bundle rebuilt at 15:48
  from the same source tree was refused twice, with the same dialog and the same
  `could not create image from rect` on stderr. This closes `specs/region-capture`'s **A1**,
  and the answer is the pessimistic one the spec anticipated.
  **Why**: `scripts/bundle-app.sh` builds an unsigned, un-notarized bundle by explicit
  non-goal, so the binary is only *ad-hoc* signed. TCC then binds the grant to the code
  hash (`CDHash`), which every source change moves. The bundle **identifier** is stable
  (`app-b6f169459d22e7a1` across the rebuild) and that is a trap — it is not what TCC keys
  on, and reading it as reassurance is how this gap was mis-called once already.
  **Confirmed a fourth time, and this is how the debugging went wrong.** The bundle in
  `/Applications` was installed at 17:12 and granted; its binary was then overwritten at
  19:15 by the next `cargo build` before the chord was tried again. Six separate causes
  were proposed and eliminated over two days — bundle recreation, a signing identity, a
  stale process, responsible-process attribution, the build directory, duplicate bundle
  identifiers — and every one of those eliminations was performed *after a rebuild that
  had already voided the grant*. The variable under test was never the one being changed.
  **The remedy named first here was tried and it did not work.** A self-signed
  code-signing certificate was created in the login keychain and `bundle-app.sh` pointed
  at it; `spctl` then rejected the bundle, the trust settings were user-level only (no
  `sudo`), and the grant still did not bind. The bundle was reverted to ad-hoc. A stable
  signing identity is still the shape of the fix, but a self-signed cert in a login
  keychain is *not* a working instance of it — do not spend the afternoon again.
  **The oracle to use instead of inference.** `capture::log_screen_capture_access` calls
  `CGPreflightScreenCaptureAccess()` at startup and logs whether *this process* holds the
  grant. It was validated against a controlled negative: a shell with no grant reports
  `NOT-GRANTED` and fails both `screencapture -x file` and `-x -c` with `could not create
  image from display`. Read that line first; it answers in one line what four retracted
  theories could not.
  **Working around it today**: re-grant after every `cargo build --release`, or use
  `Cmd+Shift+4` then `Cmd+V` — image-paste writes the PNG and types its path, which is
  the same end state the chord produces.
  **A2 is closed by the same session and in the other direction**: the grant IS required.
  With it the chord works; without it every invocation form is refused — a file target and
  `-c` alike, from a bare binary, an `.app` bundle and unrelated shells. Apple's own signed
  binary performing the capture is not sufficient on its own. The clipboard destination is
  **not** a way around the grant, which is worth stating because it looked like one: under
  `-c` no file is ever written, so `verdict` reports `NoFile` whether the capture succeeded
  or was refused, and a refused clipboard capture is indistinguishable from a working one
  in the log.
  **Two traps in `screencapture` itself, since between them they cost the session.** First,
  `-d` is *"display errors to the user graphically"* — it takes no argument, and the file is
  the bare positional `file` in `screencapture [-flags] file`. Reading `-d <path>` as a
  destination flag is wrong and the argv in `capture_args` only looks like it has one.
  Second, the refusal does not name itself: `-d` raises a dialog reading "Screenshots are
  not allowed.", which names no permission, no application and no remedy, and
  `screencapture` carries ten `ScreenshotError` cases whose stderr strings do not map
  one-to-one onto it. `capture.rs`'s `tracing::warn!` on the no-file path
  exists because of this; without it the chord just silently does nothing.
- **G2** ~~Inline-image protocol undecided~~ — **resolved**: the Kitty graphics-APC
  protocol shipped (`term-core::image::parse_graphics_apc`).
- **G3** ~~Glyph atlas not yet wired into the shader~~ — **resolved**: glyphs render
  through the swash atlas (`build_glyph_instances` feeds `renderer::atlas::Atlas`),
  with 256-color/truecolor and bold/dim/italic/underline/reverse; block-font is no
  longer the rendering path.
- **G7** Bold now comes from the font file's **real** bold face, and the fallback to
  synthesis is a real path that can come back silently. `specs/real-bold-faces`
  establishes that both shipped fonts already carried faces this renderer never
  opened — Menlo Bold/Italic/BoldItalic at faces 1/2/3, Apple SD Gothic Neo Bold
  (weight 700) at face 6 — while every bold glyph was produced by `embolden`-ing the
  regular outline. On AppleSDGothicNeo that does not thicken outlines, it **erodes**
  them: 593 of 11,172 Hangul syllables lost 86–89% of their ink, 안/간/건/난 among
  them, because offsetting those contours makes the fill cancel. Recorded here beside
  G5's A1/A2 and G6 because it is the same *kind* of fact — a property of an external
  font file that this repo cannot enforce and would not be told about:

  - **A3: the second-choice fallback fonts ship no bold face at all.** Measured:
    `Supplemental/AppleGothic.ttf` is one face at weight 400, `SFNSMono.ttf` one face
    at weight 295. If Apple ever moves or removes `Menlo.ttc` or
    `AppleSDGothicNeo.ttc`, that chain entry silently returns to synthesis — and for
    Hangul the erosion comes back with it. This is `font_set.rs`'s D2 behaving
    correctly, not a defect, but it means **"bold looks wrong again" should be read
    as "check whether the first-choice font is still there"** before anything else.
    Nothing detects it: an absent style is deliberately silent (a warning would fire
    on every startup for AppleSDGothicNeo, which legitimately has no italic face),
    so only the `#[ignore]`d sweeps in `crates/renderer/tests/bold_faces.rs` would
    notice, and only when someone runs them.
  - **A4: `BOLD_EMBOLDEN_FRAC` is no longer a weight knob.** Because it is reached
    only when a file has no bold face, raising it to make bold "look heavier" now
    deepens the erosion on exactly the glyphs that are already broken. The constant
    carries this rule in its own doc comment, and D18 records the differentiability
    floor (1.15× ink, zero Hangul syllables below it) that a re-tune has to cite.

  What is guarded: `bold_faces.rs` sweeps all 11,172 syllables at ppem 14 and 28, and
  `golden_frame.rs` asserts bold cells carry more ink **in the captured frame** than
  regular ones — the pixel half, because the ink-map contract deliberately says
  *which* cells carry ink and not *how much*, and so stays green while bold is a
  ghost.
- **G8** A sidebar card's agent status now reads two more things this repo cannot pin
  (`specs/agent-card-status`). Recorded beside G5's A1/A2 and G6 because it is the same
  *kind* of fact — another program's output, read as data, with nobody to tell us when it
  changes:

  - **Antigravity's transcript vocabulary.** The card's Antigravity state classifies the
    last step of `brain/<id>/.system_generated/logs/transcript.jsonl` by its `type`
    (`PLANNER_RESPONSE`, `USER_INPUT`, the result kinds), its `tool_calls` names
    (`ask_question` blocks on the user; eight named tools are "foreground") and its `status`
    (`RUNNING` = a background task). Every list is **closed**, so a rename degrades toward
    the recency fallback — `Working` for 10 s after the last write, then `Ready` — and never
    toward a false `Waiting for input`. A renamed `ask_question` would lose its `Waiting`
    the same way. **"Antigravity never reads Waiting any more" should be read as "check
    the step names first".**
  - **Antigravity's background-work pairing** (`specs/agy-background-working`, which amends
    `agent-card-status` D13). After a tool-less reply the card reads `Working` while a
    background task or subagent the turn started has not reported back. That rests on four
    strings, matched exactly:
    - the task-start header: content beginning `Created At: …\n` and then `Tool is running as
      a background task with task id: <uuid>/task-<n>`, on a `RUNNING` step;
    - the subagent header: content beginning `Created At: …\n` (optionally `Completed At:
      …\n`) and then `Created the following subagents:`, followed by `"conversationId"` values;
    - the notice header: a `SYSTEM_MESSAGE` line `[Message] timestamp=… sender=<id>`;
    - the task text `Task id "<id>" ` (any status word after it).

    A renamed **start** string reads as nothing pending, which is the pre-feature behaviour. A
    task is finished by either completion form, so only renaming **both** leaves it pending.
    A subagent, and a `schedule` timer, have only the `sender=` header: rename it alone and
    every turn that started one reads `Working` until the agent's next step. **"agy reads
    Working after it has finished" should be read as "grep the newest `SYSTEM_MESSAGE` for
    `sender=`".** One cost is accepted, not a defect: a hand-off after `agy` launches a
    long-lived process for the user (`cargo run`, a dev server) reads `Working` until that
    process exits or the user types.
  - **Claude Code's background-work records** (`specs/claude-background-working`). A Claude
    card whose last turn ended (`end_turn`) reads `Working`, not `Waiting for input`, while
    background work started since the last human prompt has not reported back.
    `ai_monitor::activity` reads this from typed fields, not text:
    - **starts**, a closed list of `type: "user"` `toolUseResult` shapes: `backgroundTaskId`
      (a background `Bash`); `agentId` with `isAsync: true` + `status: "async_launched"` (a
      background `Agent`) or with `background: true` + `status: "forked"` (a forked skill);
      `resumedAgentId`/`revivedAgentId` (`SendMessage` resuming an agent);
    - **completions**, four forms naming the id in their first `<task-id>`: a
      `queue-operation` `enqueue` whose `content` begins `<task-notification>`; a `user`
      record with `origin.kind: "task-notification"`; an `attachment` `queued_command` with
      `commandMode: "task-notification"`; and a `TaskStop` result (`toolUseResult.task_id`),
      since stopping writes no notification;
    - **the human prompt** that bounds the turn: a `user` record that is not a tool result,
      not `isMeta`, and whose `origin.kind` is not `task-notification` or `peer`.

    Each rename degrades differently. A renamed start field reads nothing pending, which is
    today's `Waiting`. A renamed `origin.kind` value can make a notification read as a prompt,
    which ends the walk early, also `Waiting`. Only if **all three** notification forms are
    renamed does every background turn read `Working` until the next prompt. A background
    task that never reports (a dev server the user is meant to try, a process killed outside
    `TaskStop`) holds `Working` until the user types, by design (D6). **"A finished Claude
    tab reads Working" should be read as "grep the log's newest `queue-operation` for
    `<task-notification>`".**

    **The hand-back duplicate** (`specs/session-overview-ui` D27). Claude Code 2.1.28x
    delivers a background subagent's report twice: a `peer` record with `origin.handback:
    true` and `origin.from: <task id>`, which the assistant answers, then a
    `task-notification` user record for the same id, appended after that turn ended and never
    answered. When the newest message is such a notification, `activity::skip_handed_back`
    classifies the message before it instead. A renamed `origin.kind: "peer"`, `handback` or `from`
    brings back the old symptom: the card reads `Working` after a finished turn, until the user types. **"A
    finished Claude tab reads Working and its last record is a `task-notification`" should be
    read as "check the earlier `peer` record's `origin` for `handback` and `from`".**
  - **Both agents' dialog wording and layout.** `prompt.rs` matches Claude Code's permission
    dialog by two of its question, footer and chooser lines, and the Antigravity CLI's by its
    question row with the first choice (`1. Yes, …`) as the next non-blank row. Antigravity's
    footer is deliberately **not** a marker: `agy` draws `esc to cancel` under every running
    turn, so it says nothing about a dialog. A wording or layout change reads as "not
    detected": the card falls back to the log state, which cannot see a pending permission
    (`Working`). The oracle for that case is
    the `waiting-scan` debug line (`RUST_LOG=app=debug`), which dumps the matcher's whole
    20-row input byte for byte — it is how the Antigravity fixture was captured, and it
    answers "was the dialog absent, scrolled, or merely unmatched" in one line. The
    workspace-trust dialog is not matched at all until it is captured the same way. (The
    debug line goes to **stdout**: `common::logging` keeps `fmt::layer()`'s default writer, so
    `2> file` catches nothing.)
  - **Claude Code's question, review and plan screens**
    (`specs/claude-ask-dialog-waiting`). Three more of Claude Code's layouts, each matched
    by its own pair of rows from real captures (2.1.289):
    - the `AskUserQuestion` dialog: a footer starting `Enter to select` and ending `Esc to
      cancel`, on one row or wrapped onto two, with `N. Chat about this` as the nearest
      non-blank row above it;
    - its review screen, drawn before the answers are sent: `Ready to submit your answers?`
      over exactly `1. Submit answers`;
    - `ExitPlanMode`'s approval: `Claude has written up a plan and is ready to execute.
      Would you like to proceed?`, on one row or two, over `1. …`. It needs at least 47
      columns; at 46 the question takes three rows.

    A change to those words, or any non-blank row drawn between the chat row and the
    footer, reads as "not detected", and the card falls back to the log's `Working`. **"A
    question dialog reads Working" should be read as "check the `waiting-scan` line for the
    footer and chat rows"** (or the question and first-choice rows). Two layouts are known
    and unmatched: a question dialog drawn without its footer block (the binary's `Nt`
    condition, never seen in a capture) and the `Exit plan mode?` variant.
  - **The dialog subject's two strings** (`specs/session-overview-ui` D9). Once a dialog is
    matched, `prompt::blocking_subject` reads what it asks about from two more rows of the
    same 20-row tail: Claude's permission dialog by the tool-title row (`Bash command`,
    `Edit file`, …) under the last `────` rule, taking the next non-blank row as the
    command; Antigravity's by the row `Requesting permission for:`, taking the next
    non-blank row. It runs only on the scan that matched (a level held while scrolled back
    holds its subject too). A change to either layout reads as **no subject** — the
    overview's row says `Waiting for your input` instead of `Approval needed · {cmd}` — never
    as a wrong one, and the Needs-you bucket itself does not depend on it. **"The overview
    says Waiting for your input on a permission dialog" should be read as "check the
    `waiting-scan` line for the rule and title rows".**
  - **`end_turn` is not Needs you.** `activity::state_from` maps every plain assistant
    `end_turn` to `AgentState::Waiting`, so the sidebar card reads `Waiting for input` after
    any finished Claude turn, and that wording is unchanged. The overview splits
    `WaitingForInput` (D2): Needs you only with a blocking dialog on screen or the log's
    `AssistantAsksUser` (`graph_asks_user`), otherwise Idle. A card reading `Waiting for
    input` beside an overview row reading Idle is the two agreeing, not disagreeing.
  - **When Claude Code writes a `tool_use`.** Claude Code 2.1.28x writes an assistant
    `tool_use` to the session log only once the tool **returns**. While a question or plan
    dialog is up, its call is not on disk, so the log reads the previous tool's
    `tool_result` (`Busy`) and **no log rule can see a pending dialog**. That is why the
    screen rules above exist, and why `BLOCKING_TOOLS`'s Claude entries stay quiet. They are
    kept for a version that writes pending calls again. The oracle is the log's mtime: it
    stays frozen while a dialog is up (measured: 10+ minutes on a live `AskUserQuestion`). A
    record's `timestamp` is when it was created, not when it was written, so reading the
    file afterwards cannot show this. Check it live.
- **G9** Cmd chords under the 2-Set Korean input source rest on four facts about AppKit and
  winit that this repo cannot pin (`specs/korean-input-chords`). Recorded beside G5–G8 because
  they are the same *kind* of fact: someone else's behaviour, read as data, with nobody to tell
  us when it changes.

  1. **The Korean IM answers a Cmd keyDown mid-composition by committing first.** With a
     syllable marked, it calls `insertText:` with the syllable and then `doCommandBySelector:`.
     If it stopped committing, there would be no `Ime::Commit` to recover the chord from, and
     a mid-composition chord would go quiet again, with `하` still underlined. Two of three
     synthetic probe runs showed exactly that mode, which is believed to be an artifact.
  2. **winit drops the key after a commit.** `do_command_by_selector` returns early in the
     `Committed` state (winit-0.30.13 `view.rs:426-428`), so no `KeyboardInput` is emitted.
     If a later winit forwards the key, the chord arrives twice: once replayed, once as a
     `KeyboardInput`. The one-shot `replayed_ts` guard drops the second one.
  3. **`NSApp.currentEvent` is that keyDown while the commit is handled.** `replay_lost_chord`
     rebuilds the chord from it. Measured only inside the probe's `insertText`, not inside
     winit's handler; the spec's manual checklist (step 4) is the in-app confirmation. If
     AppKit changed this, the replay could rebuild the wrong key, or nothing.
  4. **winit dispatches the commit synchronously and drains its queue first-in, first-out.**
     The handler is idle inside `keyDown` (`app_state.rs:306-320`). If the commit were queued
     (a nested run loop, or re-entry), `currentEvent` could be a later event, and winit
     prints `had to queue event`, visible under `RUST_LOG=winit=debug`. If the queue were
     not first-in, first-out, a forwarded duplicate could arrive *before* the replay, and
     the one-shot `replayed_ts` guard would no longer catch it.

  Idle chords rest on a fifth, simpler fact: winit builds a Cmd chord's logical key from
  `charactersIgnoringModifiers` (the jamo), while `characters` (the Command-layer string,
  `text_with_all_modifiers`) is Roman. `input::chord_key` swaps the latter in.
  `key_without_modifiers` is no substitute, because under Korean the current *layout* source
  is `2SetHangul`.

  **The oracles:** the `chord-substitute` and `chord-replay` lines
  (`RUST_LOG=app=debug,winit=debug`, on stdout — the `winit` target carries fact 4's line),
  and `specs/korean-input-chords/composeprobe.swift`, which drives the real IM through
  `NSTextInputContext.handleEvent` in a real `NSApplication`. A bare `CGEvent` bypasses the
  input method and has cleared this bug falsely once. **"A Cmd chord does nothing while I'm
  typing Korean"** should be read as "check for a `chord-replay` line first". No line with `하`
  still underlined means fact 1 moved (the IM did not commit); no line with `하` committed
  means fact 3 or 4 moved (look for `had to queue event`); a line whose `keyCode` is not the
  key just pressed means fact 3 or 4 moved; a `chord-replay duplicate dropped` line means fact
  2 moved.

  **Still lost mid-composition, deliberately:** Cmd+Enter, Cmd+Backspace, Cmd+Escape,
  Cmd+F-keys, Cmd+PageUp/PageDown and Cmd+ForwardDelete. If the IM commits on them too
  (unmeasured for named keys), each logs only a `chord-replay skipped` line, because no Cmd
  matcher reads them. Cmd+Tab is not listed: macOS takes it before the app sees it.
- **G10** The **overview** (`specs/session-overview-ui`, Cmd+Shift+F, revising
  `specs/session-flow-graph`, which revised `specs/session-flow-viewer`) is the Agent Monitor:
  a **full-window overlay**, not a dock occupant. It **replaced** the session graph in the same
  overlay (D1): project cards, edges, `N finished` chips and the slot layout are gone, as are
  `graph_layout.rs` and `graph_panel.rs`. Policy is app-side and pure — `app::overview`
  (buckets, identity, now-doing, elapsed, tiles, banner, filter, squares) and
  `app::overview_detail` (window, ticks, lanes, segments, events) — and hands the renderer plain
  data (`renderer::overview_model`, `u64` tabs, `u32` pane pids, colour **roles**); the renderer
  lays it out in `overview_layout`, colours it from `overview_theme` and draws it in
  `overview_panel` (D20, D21). `app::flow::FlowOverlay` is the impure shell. The layer
  (`OverviewPanel`, `OVERVIEW_PANEL_Z` = 110, above the ai-monitor's 100) paints an opaque
  backdrop over the whole viewport and reserves no inset, so opening it never reflows the grid
  (G4's truncating `Grid::resize` is not involved). The right dock is **the ai-monitor's
  alone**: the session-flow-viewer's shared `RightDock`/`ToggleDock` seam was reverted, the
  monitor toggles and persists exactly as on `main`, and the overview covers an open monitor
  rather than replacing it. It is never restored open, and opening always shows the overview,
  not the detail. The `FlowOverlay` sits at priority **300**, above rename (200), and while open
  it owns the keyboard — the chord closes it from either view, `Esc` closes the overview and
  returns from the detail, every other key (and IME text) is swallowed — and, through
  `input::pointer_owner`, every press, wheel and hover in the window. That makes it a thing to
  look at, not to type beside: the sidebar stays the live surface while typing.

  A row is an **agent session**: a tab (or tmux pane) whose detected agent is any `AgentKind`
  the sidebar knows — Claude, Antigravity, Codex, Gemini or Aider (session-flow-graph R14); in
  this window only. Its status is the sidebar card's (`row_meta` / `resolve_status`), split into
  a **bucket** by two published flags and nothing else (D2, see G8's `end_turn` note). Its
  project comes from the tabmeta worker's `RepoId`, resolved from the detected agent process's
  cwd (any kind) and published with `graph_agent_cwd` only while the overlay is open; until the
  first full pass the header reads `resolving…` and no row is drawn. **Subagents, elapsed and
  the timeline are Claude and Antigravity only** (D24): they come from Claude's session log
  (`TabMeta`/`PaneMeta.log`) or from the Antigravity transcript the `agy`'s presence lock names
  (`graph_transcript`, published on graph passes only), folded by `ai_monitor::telemetry` in
  that log's own dialect. Codex, Gemini and Aider logs are not read, so those rows show `—`
  and their detail has no timeline. An Antigravity turn ends at the first tool-less reply with
  no background work pending — G8's Antigravity pairing, shared with the card, not restated.
  `Copy resume cmd` stays Claude-only. The overlay **ignores
  `[sidebar].show_agent_status`** (user decision 2026-10-22, carried): while it is open the
  worker classifies every tab's agent whatever the toggle and publishes it as
  `TabMeta.graph_agent`/`graph_task`/`graph_asks_user`, which only the overlay reads, so the
  sidebar card still shows no agent status with the toggle off. The digest's turn rule is G8's
  background-work rule, not a restatement, so the detail's main lane never reads idle while the
  pill reads Working. Four notes follow, carried from the graph and adapted; each is recorded
  here because it reads as a bug when met cold:

  - **Non-active tmux panes are log-only, and so never Needs you from a dialog** (carried D3).
    A split tab's *active* pane takes the card's own `row_meta` status, blocking-dialog
    override and dialog subject included; every other pane takes
    `resolve_status(pane.agent, false, false, None)`, because the dialog matcher reads the
    tab's grid, which is the active pane's screen only. A permission dialog pending in a
    background pane therefore reads as the log says (`Working`, or Idle after an `end_turn`);
    such a pane reaches Needs you only through `PaneMeta.graph_asks_user`. **"A pane row
    disagrees with what that pane shows" should be read as "is it the active pane?"** before
    anything else. The jump actions switch to the tab only; panes are not focusable from here.
  - **Foreground subagents appear only on return.** The overview's subagents come from the
    parent log (`flow_telemetry`), and Claude Code 2.1.28x writes an `Agent` `tool_use` only
    once the tool returns (G8). A foreground subagent is therefore first seen already finished,
    a green square; only a background launch, whose receipt returns at once, is ever drawn as
    running (orange square, `Waiting on {k} subagent(s)`). The parent session reads `Working`
    meanwhile, with no child. For the same reason "Now doing" names the newest **finished**
    tool, not the one in flight (D4).
  - **Panes, repositories and digests cost only while the overlay is open** (D18). The worker's
    `list-panes` pass, repository resolution and the `graph_*` fields run behind
    `set_graph_wanted`; the flow-telemetry thread lives from open to close and is the **only**
    accumulator built `SessionTelemetry::with_digest`, so the ai-monitor's own fold does no new
    work, its snapshot's digest fields stay empty and its draw-cache key is unchanged (D19); the
    layer builds its three atlases (Title, Meta, Display) on its first visible frame. Closing
    stops all of it — the thread is told to stop and never joined, so a close during a large
    backfill cannot stall the loop — and clears the published panes and repositories. While
    open, the model rebuilds on a key that includes the wall-clock **minute** (elapsed and
    running durations are minute-granular, never seconds), and an unchanged frame writes no
    buffer (G1). **One bounded exception runs while closed**: D9's `blocking_subject`, which
    runs only where `shows_blocking_prompt` already scanned and matched, over the same 20 rows,
    so a dialog's subject is known the moment the overlay opens. What keeps a reopen on
    `resolving…` rather than the last open's rows is the per-open **epoch**:
    `set_graph_wanted(true)` returns one, the worker publishes it with `graph_ready`, and the
    overlay accepts readiness only for its own epoch — clearing alone would lose the race
    against a close the worker has not processed yet. The filter and the expanded-row set
    survive a close within one app run (pruned on rebuild); the view and every scroll offset
    reset.
  - **A sidebar card menu survives the overlay** (session-flow-graph D5). Opening it over an
    open card menu, and closing it with `Esc` or the chord, leaves the menu as it was; only an
    ordinary overlay consuming a key still dismisses it.

  **Deferred: the compact sidebar board** (D16). The design's 380 px "keep it open beside the
  terminal" board is not built. Its only candidate homes — the right dock, returned to the
  ai-monitor alone above, or the left tab sidebar, whose card order is the user's tab order —
  each reverse a shipped decision, so it waits for the person's decision on its home and its own
  spec. The overview's model (buckets, now-doing text, squares) is built to be reused by it.
