# Spec: claude-ask-dialog-waiting

## Meta
- **Created**: 2026-10-04
- **Type**: dev
- **Status**: approved; executed 2026-10-04. Amended at verification from the D9 captures:
  D3 (two-row footer), D5 (question match, 47-column minimum) and D9 (capture method), each
  marked *Amended* in place. See `captures/README.md`.
- **Approved by**: user
- **Approved at**: 2026-10-04
- **Predecessors**: `specs/agent-card-status` (the card reads `effective_state`),
  `specs/agent-prompt-waiting` (the on-screen detector), `specs/agent-asks-user-waiting`
  (the log rule `AssistantAsksUser`, whose premise this spec finds broken)

## Goal
Card tab status: a Claude tab whose agent is blocked on an `AskUserQuestion` dialog shows
"Working" on its sidebar card. Observed 2026-10-04 01:38 UTC on the Obsidian tab (session
`f15d7fed`, Claude Code 2.1.284): the dialog "Secret storage needs Obsidian 1.11.4 or newer…"
was on screen with its `Enter to select · Tab/Arrow keys to navigate · Esc to cancel` footer,
the ai-monitor panel read IDLE 5m49s, and the card read "Working". It should read "Waiting for
input".

## Non-goals
- The card's look: skins, icons, sizes, colours and zones stay as `sidebar-weather-cards`
  built them. Only which status a card resolves to changes.
- The ai-monitor right panel (its own IDLE / header state).
- Antigravity, Codex, Gemini and Aider: their dialogs and states are `specs/agent-card-status`'s.
- Claude's tool-permission dialog (`Do you want to proceed?`): already detected and pinned by
  `REAL_DIALOG`; it is not re-opened here.
- Making Claude Code write pending tool calls to its log, or depending on its config
  (`preferredNotifChannel`, hooks): the terminal reads what is there.

## Confirmed Goal
Make a Claude tab blocked on a **question put to the human** read **Waiting for input** on its
card and appear in the waiting band:

- **`AskUserQuestion`**, in both forms Claude Code draws: a single question, and the
  multi-question form with a tab strip (`← ☐ … ✔ Submit →`).
- **`ExitPlanMode`**'s plan-approval dialog, which has the same hole (user decision at the
  mirror: include it). Its rule ships only on a real capture; uncaptured, it stays a Known Gap
  rather than shipping on a guess.

Both are recognised **on screen** by `crates/app/src/prompt.rs`, because the session log no
longer carries a pending tool call: Claude Code 2.1.28x writes an assistant `tool_use` only
once the tool returns (measured, see Research). The stale "log-verified" claim on
`BLOCKING_TOOLS` is corrected so nobody relies on the log path again.

Inferred beyond the literal request:
- Answering the dialog returns the card to "Working" with **no clearing logic**: the detector
  is a level, and the dialog leaves the screen when answered.
- Prose that merely **quotes** the new footer (a spec, a code review, this very spec rendered
  in a tab) must not mark the tab waiting.
- The band and the card keep agreeing, since both read `sidebar_model::effective_state`.

Done when: a Claude tab on an `AskUserQuestion` dialog (single and multi-question) shows
"Waiting for input" on its card and has a band row; answering returns it to "Working"; the
same holds for `ExitPlanMode` if its capture is taken and meets the bar, else it is a recorded
Known Gap (D9's exit names which captures are mandatory); every matcher rule that ships is
pinned by a byte-exact fixture from a real `waiting-scan` capture;
`cargo build`, `cargo test --workspace` and
`cargo clippy --workspace --all-targets -- -D warnings` are green; and `cargo mutants` on the
changed files leaves no survivor that is not argued equivalent.

## Research

### Why the card read "Working" (measured 2026-10-04, 01:32–01:42 UTC)
- **The log does not carry a pending tool call.** Claude Code 2.1.28x writes an assistant
  `tool_use` line, with the thinking/text before it, only **once the tool returns**. Two
  observations:
  - The Obsidian session `f15d7fed` (2.1.284) with its `AskUserQuestion` dialog on screen: the
    log's mtime stayed at 01:32:12 for 10+ minutes. Its tail ends at the previous `Edit`'s
    `tool_result` (01:32:02.244), `hook_success`, five metadata lines (`last-prompt`,
    `ai-title`, `mode`, `permission-mode`, `atis-latch`), `total_tokens_reminder` and
    `deferred_tools_record` (01:32:12.031, the panel's "Last update 18:32:12" PDT). No
    `AskUserQuestion` record exists for the open dialog.
  - This session (2.1.287): a 0.6 s Bash call was absent from its own log while it ran, and
    appeared together with its `tool_result` afterwards.
  - A record's `timestamp` is when it was *created*, not when it was *written*. So the earlier
    `AskUserQuestion` in the same log (created 22:51:25, answered 01:31:35) cannot show when it
    reached disk, and neither could the corpus `specs/agent-asks-user-waiting` sampled.
- **So the classifier reads the previous tool's result.** `ai_monitor::activity::last_meaningful`
  (`crates/ai-monitor/src/activity.rs:202`) skips role-less lines and lands on the `Edit`
  `tool_result`, which is `UserToolResult`. `tabmeta::activity::state_from`
  (`crates/app/src/tabmeta/activity.rs:66`) maps that to `Busy`, and `resolve_status` maps
  `Busy` to `CardStatus::Working` (`crates/app/src/sidebar_model.rs:223`).
- **The log rule built for this case cannot fire.** `BLOCKING_TOOLS = ["AskUserQuestion",
  "ExitPlanMode"]` (`crates/ai-monitor/src/activity.rs:255`) → `AssistantAsksUser` → `Waiting`
  needs the pending `tool_use` to be the last meaningful line. Under 2.1.28x it is never on disk
  while pending, and once written it is immediately followed by its `tool_result`. Its doc
  comment ("log-verified", `:247-251`) checked the record's shape, not when it was written.
  `AgentState::Waiting`'s doc (`crates/app/src/tabmeta/mod.rs:132-140`) restates the same
  premise.
- `LastMessage::AssistantAsksUser` itself is **still live for Antigravity**: its own
  `BLOCKING_TOOLS = ["ask_question"]` (`crates/ai-monitor/src/antigravity/tail.rs:58`), because
  Antigravity writes the dispatching `PLANNER_RESPONSE` before the answer
  (`specs/agent-card-status` Research). The variant cannot be removed.

### The on-screen detector, and why it misses the dialog
- `prompt::shows_blocking_prompt` (`crates/app/src/prompt.rs:66`) trims trailing blank rows,
  takes the last `TAIL_ROWS` = 20 content rows (`:53`), and ORs two rules that are never pooled
  (agent-card-status D5): `claude_dialog` (`:80`, 2 of 3 markers) and `antigravity_dialog`
  (`:99`, question row with the first choice as the next non-blank row).
- Claude's three markers: question `Do you want to proceed?` / `Do you want to make this edit?`
  (`:122`, exact trimmed line), footer containing both `Esc to cancel` and `to amend` (`:129`),
  and selected choice `❯ N.` with N in 1–9 (`:135`).
- On the Obsidian screen only the selected choice matched (`❯ 1. Require 1.11.4 (Rec.)`). The
  question is free text, and the footer `Enter to select · Tab/Arrow keys to navigate · Esc to
  cancel` has no `to amend`. One of three is below `REQUIRED_MARKERS` (`:59`).
- The rows the screenshot shows, top to bottom (17 content rows from the tab strip to the
  footer, so within the 20-row tail):
  ```
  ← ☐ Min version  ☐ Missing sign-in  ✔ Submit →
  (blank)
  │ Secret storage needs Obsidian 1.11.4 or newer (yours are 1.12+). If someone runs an
  │ older Obsidian, what should this plugin version do?
  (blank)
  ❯ 1. Require 1.11.4 (Rec.)
       Set the plugin's minimum Obsidian version … (3 rows)
    2. Fallback to data.json
       Keep supporting old Obsidian … (2 rows)
    3. Type something.
  ──────────────────────
    4. Chat about this
  (blank)
  Enter to select · Tab/Arrow keys to navigate · Esc to cancel
  ```
  Transcribed from a **screenshot**, so it is not a fixture: a screenshot cannot pin glyphs,
  spacing or trailing blanks (the agent-card-status D12 rule).

### What the Claude Code binary says about these dialogs (2.1.289, `strings`-level only)
Read from `~/.local/share/claude/versions/2.1.289` to learn **which screens need capturing**.
It does not supply markers; captures do.
- **Question dialog footer** is assembled from key chords: `Enter to select`, then
  `↑/↓ to navigate` when there is one question or the literal `Tab/Arrow keys to navigate` when
  there are several, then an optional `ctrl+g to edit in <editor>`, then `Esc to cancel`, joined
  with ` · `. It is drawn **together with** a separator and the `N. Chat about this` row, in one
  block, under a condition (`!Nt`, `Nt = Xe()`, not resolvable statically). When that condition
  is false the block is not drawn at all: `Chat about this` becomes an inline option for a
  single-select question, and no footer is drawn by this component.
- The free-text option reads `Type something.` (single-select) or `Type something`
  (multi-select).
- The multi-question **review screen** is a separate view: `Review your answers`,
  `Ready to submit your answers?`, and a `Submit answers` / `Cancel` chooser. Its footer is not
  visible in the strings.
- **ExitPlanMode** draws a titled box `Ready to code?` with `Here is Claude's plan:` and the plan
  body, then a bordered section: the dimmed line `Claude has written up a plan and is ready to
  execute. Would you like to proceed?` (80 characters; measured at T1, the section's text area is 6 columns narrower than the grid, so it wraps below 86 columns), a chooser,
  and only an optional `ctrl+g to edit in <editor>` row. It has **no** `Esc to cancel` footer. A
  second variant, `Exit plan mode?` / `Claude wants to exit plan mode` with `Yes` / `No`, exists
  under a condition the strings do not explain.
- Other Claude Code pickers have footers that **start** with `↑/↓ to navigate` or `←/→ to
  switch`, not with `Enter to select` (e.g. `↑/↓ to navigate · Enter to select · ←/→ to switch ·
  Esc to cancel`).

### Where the level goes once detected (unchanged by this spec)
- `Session::rescan_dialog` (`crates/app/src/session.rs:254`) runs the matcher on
  `Terminal::viewport_tail_text(TAIL_ROWS)` (`crates/term-core/src/grid.rs:1011`, no grid clone)
  per PTY chunk. `next_dialog_level` (`:438`) gates it: no agent → false, scrolled back → hold
  the previous level, at the live bottom → scan.
- `Tabs::rescan_dialogs` (`crates/app/src/tabs.rs:298`) collects flips for the card resync and
  feeds the band each tab's `effective_state` (`crates/app/src/sidebar_model.rs:194`). The card
  resolves through the same function (`resolve_status`, `:223`), so the band and the card cannot
  disagree. A detected dialog over a `Busy` log yields `Waiting` → `CardStatus::WaitingForInput`.
- Because the level is re-evaluated on every chunk, answering (the dialog leaves the screen)
  clears it with no extra logic (`session.rs:244-250`).
- Scan diagnostics: the `waiting-scan` debug line (`crates/app/src/tabs.rs:336`) dumps
  `prompt::scan_dump` (`prompt.rs:117`), the matcher's whole input Debug-escaped. Logging
  honours `RUST_LOG` (`crates/common/src/logging.rs`) and goes to **stdout**, not stderr as an
  earlier draft said: `fmt::layer()` keeps its default writer (found at T1, where `2> file`
  caught nothing). This is how
  `specs/agent-card-status/captures/` was taken (see its `README.md`).

### Test scaffolding that exists
- Fixtures `REAL_DIALOG` (Claude permission, `prompt.rs:178`) and `REAL_ANTIGRAVITY_DIALOG`
  (`:202`), plus `REAL_ANTIGRAVITY_GENERATING` as a negative. `render_in_terminal` (`:315`)
  drives a fixture through a real `term_core::Terminal` so the matcher sees what the grid
  produces. `answering_the_dialog_clears_the_signal_in_a_real_terminal` (`:350`) is the
  level-clears pattern to copy.
- Rule tests to mirror: `a_single_marker_is_not_enough`, `the_choice_must_be_the_next_row_under_
  the_question`, `rules_do_not_pool_across_agents`, `a_prompt_scrolled_far_up_is_ignored`.

### Constraints inherited
- `term-core` stays policy-free and GPU-free (C4). `prompt.rs` is app policy reading raw
  viewport text (`specs/agent-prompt-waiting` D21).
- G1: no new periodic wake. The scan is per PTY chunk and must stay a few line predicates over
  ≤ 20 rows.
- Conservative bias: a false "waiting" is the worst failure; a missed dialog costs a band entry.
- Markers come from real captures (agent-card-status D12).
- Build/test/lint: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace
  --all-targets -- -D warnings`. Coverage is judged by `cargo mutants -f <file>` (CLAUDE.md).
- Claude Code is a moving target: 2.1.277 → 2.1.289 are installed side by side, and the user
  updated the binary at 18:38 PDT on 2026-10-03, during this investigation.

## Decisions

Complexity: **Medium** (2 signals). The first is an external resource: another program's UI
text, versioned, which changed during the investigation itself. The second is multiple actors:
Claude Code draws, the human answers, the terminal reads. Judged **not** to be signals:
- the doc edits in `ai-monitor`, `tabmeta` and `docs/architecture.md` (D7, D8), which change
  no behaviour;
- the capture session (D9), which is a process step, not a subsystem;
- D5's conditional ship, which is a scope mechanism.

**Minimum supported width: 47 columns** (*Amended* from 55 by C8). Below it the plan question
needs three rows, and D5 matches one or two. The original 55 assumed the first sentence (53
characters) would stay on one row to 55 columns. The plan section's text area is 6 columns
narrower than the grid, so at 57 the question already wraps inside that sentence. Measured: two
rows at 47 (`c8_plan_47`), three at 46 (`c8_plan_46_resized`). D3 has no fixed minimum of its
own; it matches wherever its footer fits in two rows.

### D1: The screen is the signal; the log cannot carry a pending question
- **Status**: resolved (from L1)
- **Decision**: a Claude question or plan dialog is detected **on screen** by `prompt.rs`, and
  reaches the card and the band through the existing `blocking_dialog` level and
  `effective_state`. No change to how the log is read.
- **Rationale**: under Claude Code 2.1.28x the pending `tool_use` is not on disk while the
  dialog is up (Research, two measurements), so no log rule can see it. Rejected:
  - **"Log silent N seconds after a `tool_result`" → Waiting.** Claude writes nothing while
    *any* tool runs (the 0.6 s Bash observation), so a long build or test run would look exactly
    like a question. It manufactures the worst failure.
  - **A Claude Code hook** (`PreToolUse`/`PostToolUse` matching `AskUserQuestion` and
    `ExitPlanMode`, writing a marker the terminal reads). This is the strongest alternative:
    exact, wording-independent, and proof against a UI redesign that would break the screen
    rules. The screen rules are the same class of dependency that G8 already records. The
    hook loses on three points, and the cost of choosing the screen is stated plainly
    afterwards:
    - **The terminal cannot own the hook.** It does not launch `claude`. The user types it in
      zsh, often inside tmux, whose panes take the tmux **server's** environment rather than
      the tab's, and `/worktree` starts `claude` in detached tmux sessions that belong to no
      tab at all. Neither an injected `--settings` flag nor a per-tab environment variable
      reliably reaches the process. The only route that would is a shim that wraps the user's
      `claude` command in their shell (`ZDOTDIR` or a function), and that edits the user's
      shell. Otherwise the hook lives in the user's `~/.claude/settings.json`: the agent
      configuration dependence `specs/agent-prompt-waiting` rejected the terminal-bell channel
      for.
    - **The join is the same problem.** A marker has to be tied to a tab. A per-tab id travels
      by environment, which tmux breaks (above). Session id → pid → tab is the tiered
      resolution `tabmeta::activity::resolve_log` already does for logs, so it is possible,
      but it is a second consumer of a fragile join that the screen does not need. The screen
      is already *in* the tab.
    - **A marker is latched state.** It needs a clearing event on every exit path: answered,
      `Esc`, interrupted, `ctrl+c`, session killed, terminal restarted. `PostToolUse` would
      have to be shown to fire on each of them, and any path it misses leaves a **stuck false
      "Waiting"**, the worst failure. The screen level clears itself with no exit paths to
      list (`session.rs:244-250`).

    The cost, stated plainly: the screen rules break on a Claude Code wording or layout change
    and the hook would not. The hook would also cover exactly the screens this spec may leave as
    Known Gaps (C3–C5, the `Nt` layout, an uncaptured plan dialog). That trade is accepted
    because every one of those gaps fails toward "Working", never toward a false "Waiting". D8 records the oracle that makes that failure diagnosable in one
    line. If the screen rules prove too brittle across updates, the hook (accepting the
    shim/config and latch costs above) is the next thing to try.
  - **Waiting for Claude Code to write pending calls again.** Not ours to schedule.

### D2: One dialog, one rule — the question dialog gets its own rule
- **Status**: resolved
- **Decision**: a new `claude_ask_dialog(tail)` joins the OR in `shows_blocking_prompt`, beside
  `claude_dialog` and `antigravity_dialog`. The permission rule's 2-of-3 pool is **not**
  extended with the question dialog's footer.
- **Rationale**: pooling (accepting `Enter to select … Esc to cancel` in `is_footer`) would
  detect the Obsidian screen as footer + `❯ 1.`. But the `❯` row moves with the pointer, and a
  tall question pushes it out of the 20-row tail (D4). Pooling would also let fragments of two
  different dialogs pair up (a permission question with a question-dialog footer), which
  agent-card-status D5 rules out: a screen shows one dialog, recognised by its own rule.

### D3: The question dialog = its footer, with the `Chat about this` row as the nearest non-blank row above it
- **Status**: resolved (rule shape); the exact bytes are fixed by the D9 captures
- **Decision**:
  - **Terms.** *Trimmed* means `str::trim`: Unicode whitespace only, no box or gutter glyphs.
    This is the existing rules' meaning. A **blank** row is one whose trimmed text is empty.
    Every other row is non-blank, **including a separator row** (`────`).
  - **Footer**, in one or two rows:
    - **One row**: its trimmed text **starts with** `Enter to select` and **ends with**
      `Esc to cancel`. This covers `↑/↓ to navigate` (one question), `Tab/Arrow keys to
      navigate` (several) and the optional `ctrl+g to edit in <editor>` between them, without
      enumerating the middle.
    - **Two rows (wrapped)**, *Amended* by C11: a row whose trimmed text starts with `Enter to
      select`, **immediately** followed (no blank between, because wrapped text has none) by a
      row such that the two trimmed texts joined by one space end with `Esc to cancel`. The
      original wording required the second row itself to end with `Esc to cancel`, but C11
      shows the wrap falling inside that phrase: `… · Esc to` / `cancel` at 57 columns. The
      footer's first row is the one that starts with `Enter to select`.
    - Never more than two rows. Two rows hold any footer up to roughly twice the grid width,
      minus word-wrap loss. A footer that needs three (a long editor name on a grid near the
      47-column minimum) is a Known Gap.
  - **Chat row**: trimmed, then an optional leading `❯` followed by whitespace, then exactly
    `N. Chat about this` (N one digit, 1–9, then `.`, one space, the words), with nothing
    after it.
  - **Rule**: the tail holds a footer whose first row's **nearest non-blank row above it** is a
    chat row. Only blank rows may sit between them; a separator or any other non-blank row
    breaks the pair. If the tail holds several footers, each is checked on its own and any
    one matching is enough.
  - The rule is **not** anchored to the viewport's last row, and does **not** use the `❯ N.`
    pointer.
- **Rationale**:
  - **Starts-with/ends-with, not `contains`**: a sentence that quotes the footer mid-line (a
    spec, a review, this spec rendered in a tab) does not match. Neither do the other Claude
    pickers, whose footers start differently. The binary suggested `↑/↓ to navigate` or `←/→
    to switch`. C9 shows `/model`'s `Enter to set as default · … · Esc to cancel` and
    `/resume`'s `Ctrl+A to show all projects · …`, and the workspace-trust prompt reads
    `Enter to confirm · Esc to cancel`. None starts `Enter to select`, so none reads as
    Claude waiting.
  - **Adjacency**: the binary draws, as one block, a separator, then the chat row, then a
    one-row margin, then the footer. So the nearest non-blank row above the footer is the chat
    row, with only that blank margin between, and the separator sits *above* the chat row. Either
    line can be quoted alone; the pair, in that order with only blanks between, is what prose
    does not produce. This is the same move as Antigravity's question-then-choice rule.
  - **Why a two-row clause.** The footer is 49 characters with one question and 60 with several,
    so any window narrower than that wraps it (C11 captures one). While the free-text option is
    focused, the footer gains `· ctrl+g to edit in <editor>` **if Claude Code resolves an
    editor**, which is about 100 characters with `Visual Studio Code`. The user's normal layout
    (sidebar plus ai-monitor panel) is about **88 columns**, measured from the screenshot at
    11.85 px per column. So whether the user's *own* free-text footer wraps depends on that
    editor resolution, which is unknown: `EDITOR` and `VISUAL` are unset in their shell
    (checked 2026-10-04), and the binary's fallback is unread. C5 is mandatory for that reason;
    it records what the footer actually is at 88 columns.
  - **C5's measurement** (2026-10-04, 2.1.289): with `EDITOR` and `VISUAL` unset, Claude Code
    resolves `Vim`, and the focused free-text footer reads `Enter to select · ↑/↓ to navigate ·
    ctrl+g to edit in Vim · Esc to cancel`, 73 characters, so it stays on one row at 88 columns
    (`c5_ask_freetext_typed`). With several questions it would be 84, still one row.
  - **Not the last row**: a tmux status bar or a Claude Code statusline can sit under the
    footer. Anchoring to the last row would break both, and tmux tabs are a supported pattern
    (`tabmeta/tmux.rs`).
  - **Not the pointer**: the chat row and the footer sit at the bottom of the dialog. Their
    position does not depend on how tall the options are or where the pointer is.
  - **Rejected**: footer alone (one line, quotable); footer + `Type something.` row (the
    multi-select variant drops the period, and that row sits above the separator, which breaks
    adjacency); question-text matching (the question is the model's free text).
  - **Accepted residual risk**: Claude printing **both** lines adjacently in a tab (a fixture
    quoted verbatim in a reply, `cat` of `prompt.rs` at the right offset) matches. That is the
    same exposure `REAL_DIALOG`'s question and footer have today. It is bounded the same way:
    `has_agent` gates the scan, and the level clears as soon as the text scrolls out of the
    20-row tail.
- **Capture-gated** (agent-card-status D12): if a capture shows a different shape, the rule
  follows the capture and this decision is amended at verification. That is how
  agent-card-status D5 was amended.

### D4: `TAIL_ROWS` stays 20
- **Status**: resolved
- **Decision**: no change to the tail window.
- **Rationale**: both new rules anchor on rows at the **bottom** of their dialog.
  - **Footer + chat row (D3)**: these are the last rows the question dialog draws, so a tall
    question cannot push them out. C6 is the proof: a tall question with the pointer outside
    the tail.
  - **Plan question + first choice (D5)**: below them are only the chooser's options, an
    optional `ctrl+g` row and their margins. The plan body, however long, is **above** the
    question. D4 holds while those rows stay under 20. C7 records the real count, with a plan
    deliberately taller than the viewport.

  Raising the window would widen every rule's quoting exposure, including the permission
  rule's, for no gain.

### D5: The plan dialog = its question, on one row or two, with the first choice as the next non-blank row below it
- **Status**: resolved; *Amended* at verification by C8. Shipped with C7 and C8.
- **Decision** (trimmed and blank as defined in D3):
  - **Question**, *Amended*: either one row whose trimmed text **equals** the full question
    `Claude has written up a plan and is ready to execute. Would you like to proceed?`, or two
    **adjacent** rows whose trimmed texts, joined by one space, equal it. The rule never takes
    three rows.
  - **First choice**: after the question's last row, the next non-blank row (blank rows
    between are skipped, as in D3; any other row breaks the pair), trimmed, with an optional
    leading `❯` and the whitespace after it removed, **starts with `1. `** (digit, dot,
    space). This is `is_antigravity_choice`'s shape with Claude's pointer glyph. The pointer
    sits on whichever option is selected, so option 1 can be drawn without it.
  - The `Exit plan mode?` / `Claude wants to exit plan mode` variant is **not** matched. It
    was not captured.
- **What the amendment replaced, and why.** The approved version matched a row *starting* with
  the first sentence (53 characters), plus at most one continuation row that was a
  word-boundary suffix of the full line. Its premise was that the first sentence stays on one
  row down to 55 columns. C8 disproves it: the section's text area is 6 columns narrower than
  the grid, and at 57 columns the question wraps after `ready to`, inside the first sentence
  (`c8_plan_57`). The prefix would have missed the dialog across the whole 55–58 range it was
  meant to cover. Joining two rows and comparing the **whole** question keeps every property
  the suffix clause bought:
  - a row cut mid-word joins with a space where none belongs;
  - a stray `?` or `d?` under the first row does not complete the question;
  - the continuation without its first row has nothing to join to;
  - a third row is never read.

  It also holds whatever the wrap point. The captures show the split after `Would you` at 70,
  after `ready to` at 57 and 50, and after `ready` at 47. At 46 the question takes three rows
  (`c8_plan_46_resized`), which sets the minimum at **47 columns**. A word-wrap model of the
  80-character line over a text area of `cols − 6` reproduces all five measured splits.
- **Rationale**: the plan dialog has no `Esc to cancel` footer (binary, confirmed by C7), so
  D3's shape cannot apply. Question-then-first-choice adjacency is the Antigravity rule's
  shape, which has held since agent-card-status. The plan body scrolls inside its own box
  (C7), so the question, the chooser and the `ctrl+g` row are the dialog's last rows whatever
  the plan's length. C7's tail holds 9 rows from the question down. Rejected:
  - the box title `Ready to code?`: it sits above the whole plan body, so it is usually outside
    the tail;
  - a prefix with any suffix as a continuation: `?` or `d?` would pass.

### D6: The new rules are not gated by agent kind
- **Status**: assumed (consistency with the existing rules)
- **Decision**: like `claude_dialog` and `antigravity_dialog`, the new rules run for any tab
  with a detected agent (`next_dialog_level`'s `has_agent` gate, unchanged).
- **Rationale**: the existing rules are not kind-gated, and the scan does not know the kind.
  Threading it in would change `Session::rescan_dialog`'s contract for no observed failure.
  The exposure, stated: a Gemini, Codex, Aider or Antigravity tab matches only if Claude's
  dialog rows appear on **its** screen (a quote, a `cat`). That is the same exposure the
  ungated Claude permission rule has carried since `specs/agent-prompt-waiting`, bounded the
  same way (D3's residual risk).

### D7: Keep `BLOCKING_TOOLS` and `AssistantAsksUser`; correct what they claim
- **Status**: resolved
- **Decision**: no behaviour change in `ai-monitor` or `tabmeta`. Rewrite the doc comments that
  rest on the broken premise:
  - `BLOCKING_TOOLS` (`crates/ai-monitor/src/activity.rs:236-254`): its evidence bar was the
    record's shape. Under 2.1.28x the pending call never reaches disk, so the Claude entries do
    not fire, and `prompt.rs`'s screen rules are what catch these dialogs.
  - `AgentState::Waiting` (`crates/app/src/tabmeta/mod.rs:132-140`), to the same effect.
- **Rationale**:
  - Antigravity uses the variant through its own list.
  - The Claude entries are harmless: a pending call is never on disk, and once it is written
    its `tool_result` follows it, so they can never fire wrongly.
  - They come back to life if a Claude Code version writes pending calls again. Deleting them
    buys nothing and loses that.
  - The comments must change because a reader who trusts "log-verified" would wire the next
    feature to a dead signal. This bug is what happens when they do.

### D8: Record the two external facts beside G8, with their oracles
- **Status**: resolved
- **Decision**: extend `docs/architecture.md` **G8**, which already lists "both agents' dialog
  wording and layout", with:
  - **(a)** the question and plan dialogs' markers (D3, D5) as further Claude Code UI
    assumptions.
  - **(b)** the log-write timing: Claude Code writes a `tool_use` only when the tool returns,
    so no log rule can see a pending dialog.

  Each comes with its oracle. For (a), "a question dialog reads Working" should be read as
  "check the `waiting-scan` line for the footer and chat rows". For (b), the log's mtime is
  frozen while a dialog is up.
- **Rationale**: these are the same kind of fact as G5's A1/A2, G6 and G8: another program's
  output, with nobody to tell us when it changes. Claude Code auto-updated during this very
  investigation (2.1.287 → 2.1.289).

### D9: Captures first, driven by the user during implementation
- **Status**: resolved (user, L2 round 1)
- **Decision**: implementation starts with a capture session. The implementer builds and
  launches a debug app (`RUST_LOG=app=debug ./target/debug/app 2> <file>`) and tells the user
  what to ask Claude; the user clicks through. Each screen's `waiting-scan` tail is decoded into
  `specs/claude-ask-dialog-waiting/captures/<name>.txt`, with a `README.md` in the form of
  agent-card-status's (version, width, what was on screen, any redaction). Prompts use neutral
  content. Capture list:

  | # | Screen | Proves |
  |---|---|---|
  | C1 | one question (`↑/↓ to navigate` footer) | D3 positive |
  | C2 | several questions, a question tab (`Tab/Arrow keys to navigate`) | D3 positive |
  | C3 | several questions, the `Submit` review screen | D3 or Known Gap |
  | C4 | a multi-select question | D3 or Known Gap |
  | C5 | typing a free-text answer under `Type something.`, at the user's normal ~88-column layout | D3 one- or two-row footer, or Known Gap; records whether `ctrl+g` appears and whether the footer wraps |
  | C6 | a tall question: 4 options with long descriptions, pointer on 1 | D3 + D4. **Accepted only if** the decoded tail does not contain the `❯ 1.` row, i.e. the pointer really is outside the 20-row tail; otherwise re-take it taller |
  | C7 | plan approval at ≥ 86 columns (written as ≥ 82 before C8 measured the 6-column inset), with a plan taller than the viewport | D5 positive, D4 row count |
  | C8 | plan approval at 47–85 columns (wrapped question; the range was 55–81 before C8 moved the minimum) | D5 wrapped question, minimum width (taken at 57 and 47, re-rendered at 70, 50, 46 and 45) |
  | C9 | user-opened pickers (`/model`, `/resume`) in a Claude tab | D3 negative |
  | C10 | the screen right after answering C1 | clears (level) |
  | C11 | C2 in a window narrowed until the footer wraps (planned 55–59; taken at 57) | D3 two-row footer |

  **Every capture records the Claude Code version and the grid width**, and all of them are
  taken on one version. A running `claude` process cannot change version under itself, so the
  rule is: take every capture in one `claude` process. If a second process is needed, compare
  `claude --version` first; on a mismatch, re-take the earlier captures on the new version. (The Research binary notes are from 2.1.289; the Obsidian screen was
  2.1.284, and its footer bytes agree with 2.1.289's chord assembly.)

  **Exit — when the spec is done:**
  - C1, C2, C5, C6, C9, C10 and C11 are **mandatory**. C5 is mandatory as a measurement:
    its screen may still end as a Known Gap, but only after it has been captured. The question dialog is the bug the user hit,
    and it cannot end as a Known Gap. If one of those captures does not fit D3, D3 is amended to
    the capture, and the bar is unchanged: two structurally different rows in a fixed
    adjacency, with only blanks between.
  - C7 and C8 are **attempted**. With both, D5 ships whole. With C7 only, D5 ships without its
    continuation clause (D5). With neither, or with a capture that does not meet D5's bar, D5
    does not ship and the plan dialog is a Known Gap. Either way the spec is done.

  C3–C5: if a screen does not show D3's block, it gets its own rule only when its capture shows
  two structurally different rows in a fixed adjacency, the bar D3 and D5 meet. Otherwise it is
  recorded as a Known Gap: a brief "Working" while the user is mid-answer, never a false
  "Waiting".
- *Amended* at execution (user, 2026-10-04): **the captures were taken by a harness, not by
  the user at the GUI.** Asked during T1 to take them without the user, the implementer
  offered a PTY harness, tmux-driven typing into the real app, or a clean relaunch for the
  user to drive. The user chose the harness. It runs the matcher's input path out of
  `term_core` alone: zsh through `term_core::Pty` (the tab environment), output through
  `term_core::Terminal`, `viewport_tail_text(TAIL_ROWS)` logged in `scan_dump`'s format after
  every chunk, keystrokes and resizes written to the PTY. Launched with a Finder-like
  environment, so no `TMUX`, `TERM_PROGRAM` or `CLAUDE_*` reached `claude`. A GUI app launched
  from Claude Code's own shell would have leaked those. What it does not exercise (the
  window, the event loop, the `has_agent` gate) the `tabs.rs` card tests cover with the same
  bytes. All captures are from one `claude` process, 2.1.289. Method, list and outcomes are
  in `captures/README.md`. One D3/D5-style addition came out of it: C3 shows no footer, and
  its `Ready to submit your answers?` row over `❯ 1. Submit answers` meets the bar, so it got
  its own rule.
- **Rationale**: agent-card-status D12, which held: markers come from the matcher's own input,
  byte for byte, not from a screenshot or the binary. (At L2 the user rejected a headless
  harness in favour of the established path. That was superseded at execution by the user's
  own choice; see the amendment above. The rule that markers come from the matcher's input
  is unchanged.)

### D10: Tests follow the existing fixture pattern, end to end to the card
- **Status**: resolved (from L1 precedent)
- **Decision**: each captured screen becomes a `REAL_*` fixture in `prompt.rs`. Tests:
  - **Positives** through `shows_blocking_prompt` and through `render_in_terminal`
    (`prompt.rs:315`).
  - **D3 negatives**, each built by editing a captured fixture (so a pass cannot come from an
    unrelated difference): footer alone; chat row alone; a separator or other non-blank row
    between them; the footer quoted mid-sentence; a picker footer (C9); C11 with a blank row
    inserted between the two footer rows; a footer split over three rows.
  - **D5 positives**: C7; C7 with the blank row between question and choice confirmed skipped
    (or a blank inserted, if the capture has none); C8 (the wrapped question), **only if** C8 is
    taken.
  - **D5 negatives**: question without a choice; a non-blank row between the question and the
    choice. **Only when the continuation clause ships (C8)**: a continuation row with no
    question row above it; a continuation that is not a word-boundary suffix (`?`, `d?`); two
    continuation rows. Without the clause, those rows are covered by the plain "a non-blank row
    breaks the pair" negative.
  - **Clearing**: answering clears, in the form of `answering_the_dialog_clears_the_signal_in_a_
    real_terminal`.
  - **Card, end to end**: one test per new rule in the form of
    `a_claude_permission_dialog_reads_waiting_on_its_card` (`tabs.rs:933`). Captured bytes →
    level → row → `resolve_status` over a `Busy` log → `WaitingForInput`.
- **Rationale**: the card test is the one that proves the user-visible claim. A `prompt.rs`
  unit test alone stays green if the level never reaches the row.

### D11: tmux side-by-side panes are out of scope
- **Status**: resolved (user, L2 round 1)
- **Decision**: no per-pane splitting. A dialog in one of two side-by-side tmux panes is not
  detected (each row carries the other pane's text after `│`). A full-width tmux pane is
  detected, because D3 and D5 anchor on adjacent rows, not on the last row.
- **Rationale**: no current rule handles split panes. Fixing it for one rule would leave the
  permission and Antigravity rules inconsistent, and the failure is toward "not detected".

## Constraints
- A false "Waiting" is the worst failure. Every unmatched or uncaptured screen fails toward
  "not detected" (the card reads the log's state).
- The detector stays a **level** re-evaluated per PTY chunk, with nothing latched, expired or
  replayed (agent-prompt-waiting).
- `term-core` gains nothing. All policy stays in `crates/app/src/prompt.rs` (C4, D21).
- G1: the added work per scan is a few line predicates over ≤ 20 rows; no new wake, timer or
  grid clone.
- No new dependencies. No change to `CardStatus`, the renderer, the waiting band's model or
  `effective_state`.
- Every marker string comes from a D9 capture, not from the screenshot or the binary.
- `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
  warnings` green; `cargo mutants -f crates/app/src/prompt.rs` leaves no unargued survivor.

## Known Gaps
- **The `Nt` layout.** When the question dialog's footer/chat block is not drawn (binary:
  `!Nt`, trigger unknown), D3 cannot match and the card reads "Working". Oracle: the
  `waiting-scan` line shows a question dialog with no footer.
- **`Exit plan mode?` variant** (`Claude wants to exit plan mode`, Yes/No): not matched unless
  captured.
- ~~The plan dialog itself; C3–C5 sub-screens~~: closed at verification. C7 and C8 were
  taken and D5 shipped whole (as amended). C3 got its own rule (`claude_review_dialog`). C4
  and C5 are matched by D3 (`captures/README.md`).
- **Grids narrower than 47 columns**: the plan question takes three rows, so D5 reads
  "Working". D3 still matches there while its footer fits in two rows.
- **A footer that needs three rows** (D3): a long editor name in the `ctrl+g` hint on a grid
  near the minimum.
- **tmux side-by-side panes** (D11).
- **Claude Code wording or layout changes** silently return the card to "Working" (D8 records
  the oracle). Nothing in the repo can detect an upstream change.
- **The log-write timing may change back.** Then D7's kept entries fire again, which is
  harmless, and the screen rules still agree with them.

## Requirements

Fixture names below (`REAL_ASK_*`, `REAL_PLAN_*`) stand for the `prompt.rs` consts made from the
D9 captures of the same C-number. "Detected" means `shows_blocking_prompt` returns `true`.

### R0: A Claude tab blocked on a question reads "Waiting for input" (Confirmed Goal)

#### R0.1: Question dialog → card reads Waiting for input
- **Given**: a tab whose terminal has been advanced with the C2 capture's bytes, and whose agent
  is Claude with a log state of `Busy` (the 2.1.28x log shape)
- **When**: `Tabs::rescan_dialogs` runs and the tab's row is resolved through
  `sidebar_model::resolve_status`
- **Then**: the scan reports a flip to `true` for the tab, and the status is
  `CardStatus::WaitingForInput`

#### R0.2: Question dialog → the band is fed Waiting
- **Given**: the same tab and state as R0.1
- **When**: `Tabs::rescan_dialogs` returns its `states`
- **Then**: the tab's entry is `AgentState::Waiting`, the same `effective_state` the card used

#### R0.3: Answering returns the card to Working
- **Given**: a tab at R0.1's end state
- **When**: the terminal is advanced with the C10 capture (the screen right after answering C1)
  and rescanned
- **Then**: the scan reports a flip to `false`, and the status resolves to `CardStatus::Working`
  over the same `Busy` log

#### R0.4: Plan dialog → card reads Waiting for input (only if D5 ships)
- **Given**: a tab advanced with the C7 capture, Claude agent, log state `Busy`
- **When**: `rescan_dialogs` runs and the row is resolved
- **Then**: the status is `CardStatus::WaitingForInput`

### R1: Captures come first, byte-exact (D9)

#### R1.1: Mandatory captures exist with their provenance
- **Given**: a debug build launched with `RUST_LOG=app=debug` and stderr redirected to a file
- **When**: the user drives Claude through C1, C2, C5, C6, C9, C10 and C11
- **Then**: each screen's `waiting-scan` tail is decoded into
  `specs/claude-ask-dialog-waiting/captures/<name>.txt`, and `captures/README.md` records for
  each one the Claude Code version, the grid width, what was on screen, and any redaction

#### R1.2: One version across the set
- **Given**: captures taken across more than one `claude` process
- **When**: a later process's `claude --version` differs from the first one's
- **Then**: the earlier captures are re-taken on the new version, so the README names one
  version for the whole set

#### R1.3: C6 is accepted only if the pointer is outside the tail
- **Given**: a decoded C6 capture
- **When**: its tail is searched for the `❯ 1.` row
- **Then**: the row is absent; if present, C6 is re-taken with a taller question before it
  becomes a fixture

#### R1.4: Fixtures are verbatim
- **Given**: a decoded capture
- **When**: it becomes a `REAL_*` const in `prompt.rs`
- **Then**: its bytes equal the capture file's (blank rows, leading spaces and glyphs included),
  except a redaction named in the README

#### R1.5: Plan captures are attempted
- **Given**: the capture session
- **When**: the user puts Claude in plan mode with a plan taller than the viewport, at ≥ 86
  columns (C7), and again at 47–85 columns (C8; 55–81 before C8 moved the minimum)
- **Then**: each screen is captured and recorded as in R1.1. A screen that cannot be produced is
  recorded in the README as not taken, and D9's exit decides what ships

### R2: The question dialog rule (D2, D3)

#### R2.1: One-row footer with the chat row above it is detected
- **Given**: `REAL_ASK_SINGLE` (C1) or `REAL_ASK_MULTI` (C2)
- **When**: passed to `shows_blocking_prompt`, and separately rendered through
  `render_in_terminal`
- **Then**: detected in both paths

#### R2.2: Wrapped two-row footer is detected
- **Given**: `REAL_ASK_NARROW` (C11, 57 columns), whose footer spans two rows
- **When**: passed to `shows_blocking_prompt` and through `render_in_terminal` at its captured
  width
- **Then**: detected

#### R2.3: A tall question is detected with its pointer outside the tail
- **Given**: `REAL_ASK_TALL` (C6), whose 20-row tail has no `❯ N.` row
- **When**: passed to `shows_blocking_prompt`
- **Then**: detected

#### R2.4: Only blank rows may separate the chat row from the footer
- **Given**: C1 edited so that one non-blank row sits between the chat row and the footer, once
  as a separator (`────`) and once as text
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: neither is detected; C1 with an extra blank row inserted there is detected

#### R2.5: One marker alone is not a dialog
- **Given**: C1 with the chat row removed, and C1 with the footer removed
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: neither is detected

#### R2.6: A footer quoted inside a sentence is not a footer
- **Given**: C1 with its footer row replaced by `the hint reads Enter to select · ↑/↓ to
  navigate · Esc to cancel here`, and a version with trailing text after `Esc to cancel`
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: neither is detected

#### R2.7: User-opened pickers are not a question dialog
- **Given**: the C9 captures (`/model`, `/resume` in a Claude tab)
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: none is detected

#### R2.8: The two-row clause is exactly two adjacent rows
- **Given**: C11 with a blank row inserted between its two footer rows, and C11 edited so the
  footer spans three rows
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: neither is detected

#### R2.9: Rows under the footer do not break the rule
- **Given**: C1 with a tmux-style status row appended below the footer
- **When**: passed to `shows_blocking_prompt`
- **Then**: detected

#### R2.10: The permission rule is unchanged and not pooled
- **Given**: `REAL_DIALOG`, and a screen whose only markers are the permission question
  `Do you want to proceed?` and the question dialog's footer
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: `REAL_DIALOG` is detected; the mixed screen is not

#### R2.11: Sub-screens are decided by their captures
- **Given**: the C3 (review screen), C4 (multi-select) and C5 (free-text typing at ~88 columns)
  captures
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: each is either detected (by D3, or by its own rule meeting D9's bar, with its own
  positive and negative tests) or listed in Known Gaps with the reason its capture did not fit.
  Each outcome is recorded in `captures/README.md`

### R3: The plan dialog rule (D5) — only if C7 is taken and meets the bar

#### R3.1: Unwrapped plan dialog is detected
- **Given**: `REAL_PLAN_WIDE` (C7)
- **When**: passed to `shows_blocking_prompt` and through `render_in_terminal`
- **Then**: detected

#### R3.2: Blank rows between question and first choice are skipped
- **Given**: C7, plus C7 with one extra blank row inserted between the question and the first
  choice
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: both are detected

#### R3.3: Wrapped plan question is detected
- **Given**: `REAL_PLAN_NARROW` (C8 at 57 columns) and `REAL_PLAN_MIN` (C8 at 47, the minimum),
  whose questions span two rows split at different words
- **When**: each is passed to `shows_blocking_prompt` and through `render_in_terminal` at its
  captured width
- **Then**: detected

#### R3.4: The plan rule's pair is strict
- **Given**: C7 with the first choice removed, and C7 with a text row inserted between the
  question and the first choice
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: neither is detected

#### R3.5: The two-row question is strict (*Amended* with D5)
- **Given**: C8 with its question's first row removed (the continuation alone); C8 with the
  continuation replaced by `?` and by `d?`; C8 with the question split over three rows; and the
  real 46-column screen `REAL_PLAN_TOO_NARROW`, whose question takes three rows
- **When**: each is passed to `shows_blocking_prompt`
- **Then**: none is detected

#### R3.6: Answering the plan dialog clears the level
- **Given**: a real terminal showing C7
- **When**: it is advanced with output that replaces the dialog (the C7 session's next screen,
  or the agent's following output)
- **Then**: not detected

### R4: The tail window stays 20 rows (D4)

#### R4.1: A question dialog scrolled above the tail is ignored
- **Given**: a real terminal showing C1, then advanced with more than 20 rows of plain output
- **When**: its `viewport_tail_text(TAIL_ROWS)` is passed to `shows_blocking_prompt`
- **Then**: not detected, and `TAIL_ROWS` is still 20

### R5: Gating and scope edges (D6, D11)

#### R5.1: No agent, no detection
- **Given**: a tab whose terminal shows C1 and whose metadata reports no agent
- **When**: `rescan_dialogs` runs with `has_agent = false`
- **Then**: its level is `false`

#### R5.2: Side-by-side tmux panes fail toward "not detected"
- **Given**: C1 with every row suffixed by `│` and another pane's text
- **When**: passed to `shows_blocking_prompt`
- **Then**: not detected

### R6: The log premise is pinned and the stale claims are corrected (D1, D7)

#### R6.1: The 2.1.28x pending-dialog log tail reads Busy
- **Given**: a log tail of the shape measured in Research (an assistant `tool_use` for `Edit`,
  its user `tool_result`, then `hook_success`, `last-prompt`, `ai-title`, `mode`,
  `permission-mode`, `atis-latch`, `total_tokens_reminder` and `deferred_tools_record` lines,
  with content stripped)
- **When**: it is classified by `ai_monitor::activity` and `tabmeta::activity::state_from`
- **Then**: `last` is `UserToolResult` and the state is `Busy`. The test's comment names it as
  the reason the screen rule exists

#### R6.2: `BLOCKING_TOOLS` no longer claims a working Claude path
- **Given**: `crates/ai-monitor/src/activity.rs`
- **When**: the `BLOCKING_TOOLS` doc comment is read
- **Then**: it states that under Claude Code 2.1.28x a pending call is not on disk, that the
  Claude entries therefore do not fire and are kept for versions that write pending calls,
  and that `prompt.rs`'s screen rules catch these dialogs. "log-verified" no longer stands
  unqualified. The list and its behaviour are unchanged, and the existing `AssistantAsksUser`
  tests (Claude and Antigravity) still pass

#### R6.3: `AgentState::Waiting`'s doc matches
- **Given**: `crates/app/src/tabmeta/mod.rs`
- **When**: `AgentState::Waiting`'s doc is read
- **Then**: it no longer presents `AskUserQuestion` in the log as a live Claude signal, and
  points to the screen rule

### R7: The external facts are recorded (D8)

#### R7.1: G8 names the new dialog assumptions and their oracle
- **Given**: `docs/architecture.md` G8
- **When**: it is read
- **Then**: it lists the question dialog's footer + chat row and the plan dialog's question +
  first choice as Claude Code UI assumptions, and says that "a question dialog reads Working"
  should be read as "check the `waiting-scan` line for those rows"

#### R7.2: G8 records the log-write timing and its oracle
- **Given**: `docs/architecture.md` G8
- **When**: it is read
- **Then**: it states that Claude Code 2.1.28x writes a `tool_use` only when the tool returns,
  so no log rule can see a pending dialog, and that the oracle is the log's mtime staying
  frozen while a dialog is up

### R8: Verification gates (D10, Constraints)

#### R8.1: Build, tests and lint are green
- **Given**: the finished change
- **When**: `cargo build`, `cargo test --workspace` and `cargo clippy --workspace --all-targets
  -- -D warnings` run
- **Then**: all three exit 0

#### R8.2: Mutation coverage on the detector
- **Given**: the finished change
- **When**: `cargo mutants -f crates/app/src/prompt.rs -j 4` runs
- **Then**: it leaves 0 survivors, or each survivor is argued equivalent in the spec. The
  survivor count is recorded, not a ratio

#### R8.3: No change outside the declared surface
- **Given**: the finished change
- **When**: the diff is inspected
- **Then**: behaviour changes only in `crates/app/src/prompt.rs`; `term-core`, the renderer,
  `CardStatus`, `effective_state`, `TAIL_ROWS` and the log classifiers are unchanged apart from
  doc comments and tests, and no dependency is added

## Tasks

### T1: Capture session — every D9 screen, byte-exact, one version [infra]
- **Fulfills**: R1 (R1.1–R1.5)
- **Depends on**: (none)
- **Files**: `specs/claude-ask-dialog-waiting/captures/*.txt`, `captures/README.md`
- **Shape**: `cargo build -p app`, then `RUST_LOG=app=debug ./target/debug/app 2> <scratch>/
  scan.log`. In it, one `claude` process (note `claude --version`). The implementer tells the
  user what to ask for, with neutral content, screen by screen: C1, C2, C3, C4, C5 (at the
  normal ~88-column layout), C6 (check R1.3 before moving on), C9, C10; resize the window to
  55–59 columns for C11; plan mode for C7 (a plan taller than the viewport, ≥ 86 columns) and
  C8 (55–81 columns). Extract each screen's `waiting-scan` tail and decode the Debug-escaping
  back to text, as `specs/agent-card-status/captures/` was. Record version, width, contents and
  redactions in the README. A screen that cannot be produced is recorded as not taken.
- **Needs the user** (D9). Nothing else in the plan is unblocked by guessing at these bytes.

### T2: Question dialog rule, end to end to the card [vertical]
- **Fulfills**: R2 (R2.1–R2.11), R4.1, R5.1, R5.2, R0.1, R0.2, R0.3, R8.2 (for `claude_ask_dialog`)
- **Depends on**: T1
- **Files**: `crates/app/src/prompt.rs` (rule, predicates, `REAL_ASK_*` fixtures, unit and
  real-terminal tests), `crates/app/src/tabs.rs` (card/band end-to-end tests in the form of
  `a_claude_permission_dialog_reads_waiting_on_its_card`)
- **Shape**: `claude_ask_dialog(tail)` joins the OR in `shows_blocking_prompt`, with the
  footer (one- or two-row) and chat-row predicates per D3. Write the tests first from the
  captures, and confirm they are red before the rule exists (the project's "falsify a guard"
  habit). Decide C3–C5 per R2.11 and record each outcome in the README and, if it is a gap, in
  Known Gaps. Finish with `cargo mutants -f crates/app/src/prompt.rs -j 4` and record the
  survivor count.

### T3: Plan dialog rule, or its recorded gap [vertical]
- **Fulfills**: R3 (R3.1–R3.6), R0.4, R8.2 (final pass over the whole file), R8.1, R8.3
  (whole-change gates, run after the last code change)
- **Depends on**: T2 (same files: `prompt.rs`, `tabs.rs`)
- **Files**: `crates/app/src/prompt.rs`, `crates/app/src/tabs.rs`, and the spec's Known Gaps
  if D5 ships partly or not at all
- **Shape**: per D9's exit. With C7 and C8, the whole of D5; with C7 only, D5 without the
  continuation clause (R3.3 and R3.5 then do not apply); with neither, no rule and a Known Gap
  entry. If C8 moved the 55-column minimum, propagate the new figure (D5's preamble rule).
  Final `cargo mutants -f crates/app/src/prompt.rs -j 4` over both rules: 0 survivors, or each
  argued equivalent in the spec.

### T4: Pin the log premise and correct the stale claims [infra]
- **Fulfills**: R6 (R6.1–R6.3)
- **Depends on**: (none) — parallel with T1, T2 and T3 (no shared files, no shared interface)
- **Files**: `crates/ai-monitor/src/activity.rs` (`BLOCKING_TOOLS` doc), `crates/app/src/
  tabmeta/mod.rs` (`AgentState::Waiting` doc), `crates/app/src/tabmeta/activity.rs` (R6.1 test,
  next to the existing D16 end-to-end tests)
- **Shape**: doc comments and one test only. Behaviour unchanged; the existing
  `AssistantAsksUser` tests for Claude and Antigravity must stay green.

### T5: Record the external facts in G8 [infra]
- **Fulfills**: R7 (R7.1, R7.2)
- **Depends on**: T3 (G8 must name the rules that actually shipped, and whether D5 did)
- **Files**: `docs/architecture.md`
- **Shape**: extend G8's bullet list in its existing voice: the question dialog's markers and
  the plan dialog's (or its gap), the log-write timing, and an oracle for each.

R8.1 and R8.3 (build/test/clippy green; no change outside the declared surface) are owned by
T3 as the last code task, and every other task must also leave them true. There is no separate
final-verify task; holistic verification belongs to the execute pipeline.

## External Dependencies

### Pre-work
- **A capture session with the user** (T1, D9): about 15 minutes at the keyboard of a debug
  build, with a working Claude Code login, willing to resize the window twice (C8, C11) and to
  enter plan mode. Nothing in T2/T3 may start before it.
- Disk space and time for one `cargo build -p app` (debug) before the session.

### Post-work
- Rebuild and reinstall the app the user actually runs (`scripts/bundle-app.sh`). The fix is
  invisible until the running binary is replaced.
- G7: that rebuild voids the Screen Recording grant region-capture relies on. Re-grant it, or
  expect region-capture's chord to fail until you do.
- The first real `AskUserQuestion` after install is the live check: the card should read
  "Waiting for input" and the tab should appear in the band.

## Execution record (2026-10-04)

- **T1**: 24 screens captured by the harness (D9, *Amended*). The mandatory C1, C2, C5, C6, C9,
  C10 and C11 were all taken; C3, C4, C7 and C8 too (C8 at 57 and 47, plus re-renders at 70,
  50, 46 and 45). All from one `claude` process, 2.1.289. C6's tail holds no `❯ 1.` row
  (R1.3). See `captures/README.md`.
- **T2/T3**: `claude_ask_dialog` (D3), `claude_review_dialog` (C3) and `claude_plan_dialog`
  (D5) joined the OR in `shows_blocking_prompt`. The tests were written first against stub
  rules returning `false`: 16 failed and every negative passed. With the rules in, all pass.
  Sixteen `REAL_*` fixtures were checked byte-equal to their capture files with a temporary
  `include_str!` test (R1.4). The standing check is now `captures/check_fixtures.py`, Gate 1
  of `loop.md`. Card end-to-end tests in `tabs.rs`:
  `a_claude_question_dialog_reads_waiting_on_its_card` (R0.1–R0.3),
  `a_claude_plan_dialog_reads_waiting_on_its_card` (R0.4) and
  `a_question_dialog_on_a_tab_without_an_agent_is_not_waiting` (R5.1). Their tabs are sized
  to the captures' 88×36: an 80-column tab hard-wraps the plan question mid-word, and the rule
  correctly refuses it.
- **R8.2, mutation**: `cargo mutants -f crates/app/src/prompt.rs -j 4` (scoped to
  `-p app --lib`). The first pass left **3 survivors**, all in the pre-existing permission rule:
  `is_question → false`, `||→&&` in `is_question`, and `is_footer → false`. Every fixture
  carried all three markers or at most one, so no test could tell. They were not equivalent,
  so a test was added (`any_two_of_the_permission_dialogs_three_markers_are_enough`). Pass
  before the verification gate: **0 survivors** of 61. After the gate's fixes, which added
  `without_pointer` and the review choice, scoped to `-p app`: **0 survivors** of 66. Every caught mutant's log shows a `prompt::tests` failure,
  so no kill came from an unrelated test.
- **R8.1**: `cargo build`, `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` (local stable; CI's newer clippy may differ) and
  `cargo test --workspace` are green: 1920 passed, 0 failed, 18 ignored.
- **R8.3**: behaviour changes only in `prompt.rs`. `ai-monitor/src/activity.rs` and
  `tabmeta/mod.rs` changed comments only; `tabs.rs` and `tabmeta/activity.rs` gained tests only.
  `term-core`, `Cargo.lock` and `TAIL_ROWS` are unchanged.
- **G1**: the per-scan work is line predicates over ≤ 20 rows. The two-row question check
  strips instead of building a joined string, so it allocates nothing per row. The footer's
  join allocates only on a row that starts with `Enter to select`.
- **Verification gate (harness-ops:loop, `loop.md`)**: in iteration 1 every gate passed, and the
  independent Gate 3 checker scored 8/8/9/8/7. The checker also found defects, all inside the
  auto-fix fence, which iteration 2 fixed:
  - the review rule accepted any `1. ` row under its question, so assistant prose
    (`Ready to submit your answers?` / `1. Keep the button label short`) read as a dialog. It
    now requires the captured `1. Submit answers`, which narrows it toward its captures;
  - question equality was unpinned (a hand mutant turning `==` into `contains` survived), so
    quoted-in-a-sentence negatives were added for both questions;
  - `❯1.` was accepted as a pointer, against D3/D5's "followed by whitespace". All three
    predicates now share `without_pointer`;
  - R5.1 gained a positive control;
  - assorted doc claims were corrected (the app's 8192-byte reads, the standing
    `check_fixtures.py`, 82 → 86 columns, G8's review and break-condition wording, stale D9/D3
    rationale).

  Separately, in iteration 1, before the checker ran: falsifying `check_fixtures.py` showed that
  Claude Code's input row is `❯` followed by a no-break space (U+00A0). Three of them sat raw in
  the fixture literals, two at the end of a source line. They were spelled `\u{a0}` there
  (bytes unchanged, 16/16). In iteration 3, one-sided quotes of each question (trailing or
  leading text only) were added as negatives, and C8's range was corrected to 47–85.
