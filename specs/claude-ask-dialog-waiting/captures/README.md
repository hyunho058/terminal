# Captures (claude-ask-dialog-waiting D9, as amended)

Real Claude Code screens, each one the matcher's whole input: `viewport_tail_text(TAIL_ROWS)`
of a `term_core::Terminal`, logged in `prompt::scan_dump`'s Debug-escaped format and decoded
back to text. They are the source of truth for `prompt.rs`'s question, review and plan rules.
Markers come from these bytes, not from the Obsidian screenshot or the `claude` binary's
strings.

## How they were taken

**Harness, not the GUI app.** D9 originally had the user drive a debug app while its
`waiting-scan` line was logged. At execution the user asked for the captures to be taken
without them. The app cannot be driven from a shell, so they were taken by a throwaway
harness instead (scratch crate, not in the repo). It does the app's work on the matcher's input path:

- zsh spawned through `term_core::Pty::spawn`, so the child gets the tab environment
  (`TERM=xterm-256color`, the agent-variable denylist, `LC_CTYPE=UTF-8`, the shell-integration
  `ZDOTDIR`);
- PTY output read in 1024-byte chunks and fed to
  `term_core::Terminal::advance`. The app reads up to 8192 bytes at a time
  (`session.rs`). Chunk size changes how many intermediate frames are logged, not the
  settled screen a capture is taken from;
- after every chunk whose tail changed, `viewport_tail_text(20)` logged as
  `off=N | "<Debug-escaped>"`, i.e. `scan_dump`'s format;
- keystrokes written to the PTY master, and resizes sent to both the PTY and the grid,
  exactly as the app's resize does.

No GUI, no event loop, no `has_agent` gate. Those are covered by the card tests in `tabs.rs`,
which take these same bytes through `Tabs::rescan_dialogs` to `resolve_status`.

**Environment.** The harness was launched with `env -i` and a Finder-like environment (`HOME`,
`USER`, `LOGNAME`, `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, `SHELL`, `TMPDIR`), so no
`TMUX`, `TERM_PROGRAM` or `CLAUDE_*` variable reached `claude`. The first attempt used the GUI
app launched from Claude Code's own shell, but that leaks the parent session's
`CLAUDE_CODE_*` and `TERM_PROGRAM` into every tab, and that app was killed before any
capture. Checked inside the harness's zsh: only `TERM`, `LC_CTYPE` and `ZDOTDIR` of those
families were set.

**One process, one version.** Every capture is from one `claude` process, **Claude Code
2.1.289**, started in an empty `/tmp/ask-capture` with Opus 5.5, in auto mode (later plan
mode), on 2026-10-04 between 22:52 and 23:01 UTC. Grid height was 36 rows throughout. Widths were
changed by resizing the running harness.

**Correction to the spec's Research.** The `waiting-scan` line goes to **stdout**, not stderr:
`common::logging` installs `fmt::layer()` with its default writer. The first launch's
`2> file` caught nothing for that reason.

## The screens

`cols` is the grid width. "Resized" means the screen was re-rendered by Claude Code after the
grid was resized with the dialog already open. It was not first drawn at that width.

| File | C# | cols | On screen | Outcome |
|---|---|---|---|---|
| `c1_ask_single.txt` | C1 | 88 | One question, pointer on option 1, footer `Enter to select · ↑/↓ to navigate · Esc to cancel` | D3 positive (`REAL_ASK_SINGLE`) |
| `c2_ask_multi.txt` | C2 | 88 | Three questions, tab strip `←  ☐ Fruit  ☐ Colour  ☐ Size  ✔ Submit  →`, footer `… Tab/Arrow keys to navigate …` | D3 positive (`REAL_ASK_MULTI`), card test R0.1/R0.2 |
| `c2_ask_multi_second_question.txt` | C2 | 88 | The same dialog on its second question | Same shape, not a fixture |
| `c3_ask_review.txt` | C3 | 88 | The review screen: `Review your answers`, `Ready to submit your answers?`, `❯ 1. Submit answers` / `2. Cancel`. **No footer** | Own rule (`claude_review_dialog`): question row over exactly `1. Submit answers`, the bar D3 and D5 meet (`REAL_ASK_REVIEW`) |
| `c3_after_submit.txt` | | 88 | After submitting C3 | Not a fixture |
| `c4_ask_multiselect.txt` | C4 | 88 | Multi-select: `[ ] Cheese`…, `5. [ ] Type something` (no period), `Submit`, then separator, `6. Chat about this`, footer | D3 positive (`REAL_ASK_MULTISELECT`) |
| `c4_ask_multiselect_review.txt` | C4 | 88 | Its review screen, same shape as C3 | Matched by the review rule; not a fixture |
| `c5_ask_freetext_focused.txt` | C5 | 88 | Pointer moved to `4. Type something.`: footer gains `ctrl+g to edit in Vim` | One-row footer, D3 positive |
| `c5_ask_freetext_typed.txt` | C5 | 88 | `hello world` typed into it (`❯ 4. hello world`) | D3 positive (`REAL_ASK_FREETEXT`) |
| `c6_ask_tall.txt` | C6 | 88 | Four options with ~60-word descriptions, pointer on 1. **The tail holds no `❯ 1.` row** (R1.3 checked) | D3 + D4 positive (`REAL_ASK_TALL`) |
| `c7_plan_88.txt` | C7 | 88 | Plan approval: the plan scrolls inside its own box, then a separator, the question on one row, a blank, `❯ 1. Yes, and use auto mode`, two more options, `shift+tab to approve with this feedback`, the `ctrl+g` + plan-path rows | D5 positive (`REAL_PLAN_WIDE`), card test R0.4 |
| `c7_plan_88_after_esc.txt` | | 88 | After declining C7 with Esc | Clears (`REAL_PLAN_ANSWERED`) |
| `c8_plan_57.txt` | C8 | 57 | Plan approval drawn at 57: question wraps **after `ready to`**, inside its first sentence | D5 two-row positive (`REAL_PLAN_NARROW`) |
| `c8_plan_47.txt` | C8 | 47 | Plan approval drawn at 47: wraps after `ready` | D5 two-row positive, the minimum (`REAL_PLAN_MIN`) |
| `c8_plan_70_resized.txt` | C8 | 70 | Resized: wraps after `Would you` | Two rows; not a fixture |
| `c8_plan_50_resized.txt` | C8 | 50 | Resized: wraps after `ready to` | Two rows; not a fixture |
| `c8_plan_46_resized.txt` | C8 | 46 | Resized: **three rows** | Not detected, Known Gap boundary (`REAL_PLAN_TOO_NARROW`) |
| `c8_plan_45_resized.txt` | C8 | 45 | Resized: three rows | Not a fixture |
| `c9_model_picker.txt` | C9 | 88 | `/model`: footer `Enter to set as default · s to use this session only · Esc to cancel` | D3 negative (`REAL_MODEL_PICKER`) |
| `c9_resume_picker.txt` | C9 | 88 | `/resume` with no sessions in this project: wrapped footer `Ctrl+A to show all projects · … · Type to` / `search · Esc to cancel` | D3 negative (`REAL_RESUME_PICKER`) |
| `c10_after_answer.txt` | C10 | 88 | The screen right after answering C1 | Clears (`REAL_ASK_ANSWERED`), card test R0.3 |
| `c11_ask_multi_57.txt` | C11 | 57 | C2's dialog at 57: footer wraps as `Enter to select · Tab/Arrow keys to navigate · Esc to` / `cancel` | D3 two-row positive (`REAL_ASK_NARROW`) |
| `c11_after_esc.txt` | | 57 | After cancelling C11 with Esc | Not a fixture |
| `x_workspace_trust_dialog.txt` | | 88 | `claude`'s workspace-trust prompt on start, footer `Enter to confirm · Esc to cancel` | Extra D3 negative (`REAL_TRUST_DIALOG`) |

Every fixture in `prompt.rs` is byte-equal to its file here: 16 of 16. The standing check is
`check_fixtures.py` in this directory. It decodes each `REAL_*` literal and compares it with
the file it names, exits non-zero on a mismatch, and is Gate 1 of `../loop.md`. Run it from
the repo root. A fixture edited by hand, or a capture re-taken without regenerating its
literal, fails it. It was confirmed to fail on a one-byte change in two captures.

One encoding detail: Claude Code draws its input row as `❯` followed by a **no-break space**
(U+00A0), not an ASCII space. The literals spell it `\u{a0}`, so it is visible in the source and
no editor strips it as trailing whitespace.

## What the captures changed in the spec

- **D3's two-row footer.** The spec expected the second row of a wrapped footer to end with
  `Esc to cancel`. C11 shows the wrap falling *inside* that phrase (`… · Esc to` / `cancel`).
  The rule now joins the two rows with one space and applies starts-with/ends-with to the
  result. It still takes exactly two adjacent rows.
- **D5's question.** The spec expected the first sentence (53 characters) to stay on one row
  down to 55 columns. The plan section's text area is 6 columns narrower than the grid, and at
  57 the question wraps after `ready to` (C8). The prefix + word-boundary-suffix design is
  replaced by: the full question on one row, or two adjacent rows whose trimmed texts joined by
  one space **equal** it. Measured: two rows down to **47 columns**, three rows at 46 (a
  word-wrap model of the 80-character line reproduces all five measured wrap points).
- **C3 got its own rule.** The review screen draws no footer. Its `Ready to submit your
  answers?` row over `1. Submit answers` meets D9's bar. The choice is matched exactly: an early
  version accepted any `1. ` row, and the Gate 3 checker showed that an agent's own prose with
  that question over a numbered list would read as a dialog. The same screen follows a single
  multi-select question (`c4_ask_multiselect_review`).
- **C5 is not a gap.** With `EDITOR`/`VISUAL` unset, Claude Code still resolves `Vim`. The
  free-text footer is 73 characters at 88 columns, so one row.

## Not taken

- The `/resume` picker with sessions listed. Pressing `Ctrl+A` would have shown the user's own
  session titles from other projects. The empty-list screen already carries the picker's
  footer.
- `Exit plan mode?` / `Claude wants to exit plan mode` (D5's second variant). It did not appear;
  it stays a Known Gap.
- D3's `Nt` layout, where the footer/chat block is not drawn. It did not appear in any screen
  here.

## Redaction

`c11_ask_multi_57.txt`, row 2: the plan path's username is replaced with `anonymous`, which has
the same length, so the row wraps exactly as captured. That row plays no part in detection.
Nothing else is edited.
