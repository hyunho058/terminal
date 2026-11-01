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
| **right dock** | The band at the window's right edge that `Renderer::right_inset` reserves. The ai-monitor's alone: it opens, closes and persists there exactly as on `main` (`AppRequest::SetMonitorOpen`, `OverlayCommand::RestoreMonitor`). The overview does not share it — it covers it (session-flow-graph D4, carried). |
| **overview** | The Agent Monitor (`Cmd+Shift+F`, `specs/session-overview-ui`, replacing the session graph): a **full-window** overlay (`renderer::overview_panel::OverviewPanel`, z 110) drawn over the terminal, sidebar and an open ai-monitor without reflowing anything. Header `Agent Monitor` with `{n} sessions · {m} subagents` and filter tabs `All / Needs you / Working / Idle`; four tiles (Needs you, Working, Idle, Subagents); a `Waiting on you` banner naming the first Needs-you row, with `Open session →`; then **one row per agent session** — a tab or tmux pane running any detected agent (Claude, Antigravity, Codex, Gemini, Aider), in this window only — showing project name + agent tag, branch, now-doing text over the latest prompt, subagent squares (`N running · M done`, expandable into a tree) and elapsed since the latest human prompt. Rows sort by **bucket**, then strip-then-pane order (D2). Subagents, elapsed and the timeline exist for Claude **and Antigravity** sessions (D24): Claude's session log and Antigravity's transcript both record them; Codex, Gemini and Aider logs are not read. The header reads `resolving…` and no row is drawn until the worker's first full pass for this open. A row's state is the sidebar card's, through `row_meta`; the overview classifies nothing, it only splits one status into buckets. While open it owns every key (`Esc` and the chord act; IME text is swallowed), press, wheel and hover. A row press opens its **session detail**; the row's `→`, the banner's button and the detail's `Focus terminal` switch to the session's tab (never a pane) and close. Never restored open. Shipped. |
| **bucket** | An overview row's status class (D2): **Needs you** = `WaitingForInput` **and** (a blocking dialog on screen — `SidebarRow.blocking_dialog` — **or** the log's last meaningful message is `LastMessage::AssistantAsksUser`, published graph-only as `TabMeta`/`PaneMeta.graph_asks_user`); **Working** = `Working`; **Idle** = `Ready`, or `WaitingForInput` with neither signal; **Agent** = `Hidden` (agent present, state unknown — counted in `All` and `{n}`, in no tile, sorted last, never shown as ready). A Claude turn that merely ended (`end_turn`) is **Idle** in the overview even though its sidebar card reads `Waiting for input`: the card keeps its wording, the bucket refines it. A non-active tmux pane can be Needs you only through `graph_asks_user`, never through a dialog. |
| **session detail** | The overlay's second view, one session (D10, D12, D13): a header card (project, agent tag, status pill, branch, worktree, elapsed, subagent counts, `Focus terminal`, and `Copy resume cmd` for a Claude session with a known id); a **timeline** over the last 60 minutes (or since the session's first event) with a main lane split into *working* / *waiting on subagent* / *idle* and one bar lane per subagent; a selected-subagent panel (default: newest; type, status, start, duration, first four lines of its report); and the newest 8 recent events. `Esc` or `‹ All sessions` returns to the overview with its scroll, filter and expanded rows intact; a session that disappears returns the view to the overview. A Codex, Gemini or Aider session shows the header and `No timeline — this agent's log is not read.`; an Antigravity session has a timeline but no `Copy resume cmd`. |
| **session digest** | What the overview reads from one Claude log or Antigravity transcript (D19, D24): `app::flow_telemetry::SessionDigest` — subagents (with `report_preview`), `last_tool` / `last_work_tool` (name, one-line summary, start), `last_prompt_ms`, `turns` (a turn ends only when the assistant has ended it **and** no background work started in it is pending — G8's rule), `events`, `first_event_ms`, `session_id`. An Antigravity transcript (the worker's graph-only `graph_transcript`) yields the same fields from its steps: `USER_INPUT` is the prompt, a turn ends at the first tool-less `PLANNER_RESPONSE` with no background work pending (G8's Antigravity pairing), `invoke_subagent` is the Agent-like call, and there are no report previews. Folded by `ai_monitor::telemetry` only when the accumulator is built with `SessionTelemetry::with_digest`, which **only** the overlay's flow-telemetry thread sets; the ai-monitor's own fold leaves it off and does no digest work. |
| **dialog subject** | What a blocking dialog on screen asks about (D9): `DialogSubject::{Command(String), Question, Plan}`, read by `prompt::blocking_subject` from the same tail as `shows_blocking_prompt`, only where that matched, and published beside the boolean as `SidebarRow.blocking_subject` (`None` whenever `blocking_dialog` is false). Drives the Needs-you now-doing text (`Approval needed · {cmd}`, `Question waiting`, `Plan approval waiting`; none → `Waiting for your input`). The sidebar card does not draw it. |
| **repo identity** | `RepoId { common, main_root, linked }`, resolved by the tabmeta worker from the **detected agent process's** cwd, any kind (not the shell's, G5), and published only while the overview is open. Names an overview row's project (basename of `main_root`; outside any repository, the folder name of the agent's cwd, published as `graph_agent_cwd`), and `linked` adds the worktree directory's basename after the branch, which tells two same-project rows apart. Rows are no longer grouped by it. |

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
